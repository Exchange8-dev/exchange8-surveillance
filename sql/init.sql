-- SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
-- SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
-- SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Surveillance
-- SPDX-License-Identifier: Apache-2.0

-- Ex8:Surveillance — detection alerts, case management, ops alerts.
--
-- Design: services/surveillance/DESIGN.md §3 (data model).
-- Applied by `sentin8-cli vertical init`.

CREATE SCHEMA IF NOT EXISTS surveillance;

-- Detection alerts (pattern, events, confidence).
CREATE TABLE IF NOT EXISTS surveillance.surveillance_alert (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    pattern     TEXT NOT NULL CHECK (pattern IN
                ('wash_trade','spoofing','layering','momentum_ignition',
                 'insider_trading','front_running','marking_the_close',
                 'excessive_crossing')),
    instrument_id UUID,
    account_id  TEXT,
    confidence  SMALLINT NOT NULL CHECK (confidence BETWEEN 0 AND 100),
    evidence    JSONB NOT NULL DEFAULT '[]',
    ts          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_exchange8_svalert_env ON surveillance.surveillance_alert (env_id, pattern);

-- Case management (open/investigating/closed).
CREATE TABLE IF NOT EXISTS surveillance.surveillance_case (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    alert_id    UUID NOT NULL REFERENCES surveillance.surveillance_alert(id),
    state       TEXT NOT NULL DEFAULT 'open' CHECK (state IN
                ('open','investigating','closed')),
    dismissed   BOOLEAN NOT NULL DEFAULT FALSE,
    assigned_to TEXT,
    opened_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at   TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_exchange8_svcase_env ON surveillance.surveillance_case (env_id, state);

-- Ops alerts: admin unacked operational events.
CREATE TABLE IF NOT EXISTS surveillance.alert (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    message     TEXT NOT NULL,
    source      TEXT NOT NULL,
    admin_id    TEXT,                    -- NULL = unacked
    ack_time    TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_exchange8_alert_env_unacked ON surveillance.alert (env_id) WHERE admin_id IS NULL;

-- ---------------------------------------------------------------------------
-- RLS (architecture-principles.md §4.3): enable + force on all tenant tables.
-- Policies use the platform helpers sentin8_context_initialized() and
-- sentin8_current_env_id(); services call set_tenant_context on the same
-- connection as the data query.
-- ---------------------------------------------------------------------------
DO $$
DECLARE tbl TEXT;
BEGIN
    FOREACH tbl IN ARRAY ARRAY['surveillance.surveillance_alert', 'surveillance.surveillance_case', 'surveillance.alert'] LOOP
        BEGIN
            EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', tbl);
            EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_select ON %2$s;
CREATE POLICY %1$I_select ON %2$s FOR SELECT USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_insert ON %2$s;
CREATE POLICY %1$I_insert ON %2$s FOR INSERT WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_update ON %2$s;
CREATE POLICY %1$I_update ON %2$s FOR UPDATE USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
            EXECUTE format('DROP POLICY IF EXISTS %1$I_delete ON %2$s;
CREATE POLICY %1$I_delete ON %2$s FOR DELETE USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())', split_part(tbl, '.', 2), tbl);
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'RLS setup failed for %: %', tbl, SQLERRM;
        END;
    END LOOP;
END $$;

-- ---------------------------------------------------------------------------
-- Per-service role grants (schema-per-service: this service owns schema
-- `surveillance`; grants live in db `exchange8` only — design doc §1).
-- ---------------------------------------------------------------------------
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'exchange8_surveillance_service') THEN
        ALTER ROLE exchange8_surveillance_service SET search_path = surveillance, public;
        GRANT USAGE ON SCHEMA surveillance TO exchange8_surveillance_service;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA surveillance TO exchange8_surveillance_service;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA surveillance TO exchange8_surveillance_service;
    END IF;
END $$;

-- ---------------------------------------------------------------------------
-- Local outbox (DBX §6.7 — generated from `sentin8_outbox::outbox_table_sql(
-- "surveillance")`): rows written in the same DB transaction as the domain change;
-- the shell's relay (boot step §8b) pushes them to Bus `POST /v1/produce`.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS surveillance.outbox (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    event_type      TEXT NOT NULL,
    stream_family   TEXT NOT NULL,
    payload         JSONB NOT NULL,
    publisher_id_token  TEXT,
    publisher_subject   TEXT,
    relayed         BOOLEAN NOT NULL DEFAULT false,
    relayed_at      TIMESTAMPTZ,
    relay_attempts  INT NOT NULL DEFAULT 0,
    next_retry_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_surveillance_outbox_unrelayed
    ON surveillance.outbox (created_at) WHERE relayed = false;
DO $$ BEGIN
    EXECUTE 'ALTER TABLE surveillance.outbox ENABLE ROW LEVEL SECURITY';
    EXECUTE 'ALTER TABLE surveillance.outbox FORCE ROW LEVEL SECURITY';
    EXECUTE 'DROP POLICY IF EXISTS outbox_select ON surveillance.outbox';
    EXECUTE 'CREATE POLICY outbox_select ON surveillance.outbox FOR SELECT
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
    EXECUTE 'DROP POLICY IF EXISTS outbox_insert ON surveillance.outbox';
    EXECUTE 'CREATE POLICY outbox_insert ON surveillance.outbox FOR INSERT
        WITH CHECK (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
    EXECUTE 'DROP POLICY IF EXISTS outbox_update ON surveillance.outbox';
    EXECUTE 'CREATE POLICY outbox_update ON surveillance.outbox FOR UPDATE
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'RLS setup failed for surveillance.outbox: %', SQLERRM;
END $$;

CREATE OR REPLACE FUNCTION surveillance.surveillance_notify_outbox() RETURNS TRIGGER
LANGUAGE plpgsql AS $fn$
BEGIN
    PERFORM pg_notify('surveillance_outbox_changed', '');
    RETURN NEW;
END;
$fn$;
DROP TRIGGER IF EXISTS outbox_notify_trigger ON surveillance.outbox;
CREATE TRIGGER outbox_notify_trigger
    AFTER INSERT ON surveillance.outbox
    FOR EACH ROW
    EXECUTE FUNCTION surveillance.surveillance_notify_outbox();

-- ---------------------------------------------------------------------------
-- Local audit journal (DBX §6.8 — generated from
-- `sentin8_audit::audit_journal_sql("surveillance")`): rows written in the same
-- transaction as the domain change; the shell's pusher (boot step §8c) pushes
-- them to Sys `POST /v1/audit` (`sentin8:sys:audit:write`). A Sys outage
-- never drops audit.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS surveillance.audit_journal (
    id              UUID PRIMARY KEY,
    env_id          UUID NOT NULL,
    action          TEXT NOT NULL,
    target_resource_type TEXT,
    target_resource_id    TEXT,
    actor_user_id   TEXT,
    actor_subject_type TEXT,
    severity        TEXT NOT NULL DEFAULT 'info',
    correlation_id  TEXT,
    before_state    JSONB,
    after_state     JSONB,
    details         JSONB,
    pushed          BOOLEAN NOT NULL DEFAULT false,
    pushed_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_surveillance_audit_journal_unpushed
    ON surveillance.audit_journal (created_at) WHERE pushed = false;
DO $$ BEGIN
    EXECUTE 'ALTER TABLE surveillance.audit_journal ENABLE ROW LEVEL SECURITY';
    EXECUTE 'ALTER TABLE surveillance.audit_journal FORCE ROW LEVEL SECURITY';
    EXECUTE 'DROP POLICY IF EXISTS audit_journal_dml ON surveillance.audit_journal';
    EXECUTE 'CREATE POLICY audit_journal_dml ON surveillance.audit_journal
        USING (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())
        WITH CHECK (sentin8_context_initialized()
               AND env_id = sentin8_current_env_id())';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'RLS setup failed for surveillance.audit_journal: %', SQLERRM;
END $$;

-- Outbox + journal role grants (same pattern as the table grants above).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'exchange8_surveillance_service') THEN
        GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA surveillance TO exchange8_surveillance_service;
    END IF;
END $$;
