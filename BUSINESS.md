# BUSINESS.md: Exchange8:Surveillance — Business Requirements & Functionality

> Audience: product owners, business analysts, and anyone evaluating what
> Exchange8:Surveillance **does for the business**. For architecture read
> `README.md`; for wire-level detail read `DESIGN.md`.

## 1. Business Purpose

- Keep the venue **compliant with market-abuse regulation** (MAR / MiFID II):
  manipulative trading patterns are watched continuously, and every detection
  becomes a defensible, auditable record.
- Turn raw trading activity into **reviewable cases**: each alert carries the
  pattern, the account, the confidence and the supporting evidence, so a
  compliance officer can open, investigate, and close it with a recorded
  disposition.
- Let the compliance desk **work by severity**: insider-like trading and
  front-running outrank spoofing-family patterns, which outrank close-window
  and self-crossing volume — the triage view always exposes the most severe
  open cases first.
- **Separate routine trading from abuse**: detectors require the pattern's
  actual signature (repeated self-crosses, a cancel burst, an outsized
  close-window trade), so ordinary market participants are not flagged for
  one accidental self-cross or a balanced order flow.
- Surface **operational alerts** (the venue's alert log) with an
  acknowledgement trail so nothing operational goes unseen.

## 2. Business Scope (What the Service Provides)

| # | Capability | Description |
|---|---|---|
| 1 | Market-abuse pattern detection | Eight regulated patterns recognized over a normalized observation stream, with a stable pattern vocabulary that matches the persisted alert rows exactly |
| 2 | Wash-trade detection | Counts repeated self-crosses per account and instrument; fires once at the configured threshold |
| 3 | Spoofing detection | Cancels-without-execution bursts against the account's order activity, with a computed confidence |
| 4 | Marking-the-close detection | Outsized notional executions inside the configured close window, relative to the session's median trade |
| 5 | Severity triage | Every pattern carries a regulatory severity rank; open cases surface highest-severity first, stable on ties |
| 6 | Case management | A per-alert workflow (open → investigating → closed) with dismissal recording and monotonic case identifiers |
| 7 | Evidence capture | Each alert embeds the supporting observations so the case file is self-contained for review |
| 8 | Operational alert queue | Unacknowledged ops alerts with an acknowledgement (admin + time) trail |
| 9 | Admin surfaces | Two RFW admin pages (alerts, cases) registered with the platform admin shell |
| 10 | Audit & events | Every state change journaled and relayed to Sentin8:Bus (outbox producer) and the platform audit ingest |

**What the service does NOT own (business boundary)**

| Concern | Owner |
|---|---|
| Regulatory report generation from case outcomes | Exchange8:Regulatory |
| The trade/execution event stream this service observes | Exchange8:ME / Exchange8:Clearing / Exchange8:Gateway / Exchange8:OMS |
| Order acceptance decisions and pre-trade protection | Exchange8:Risk / market protection in Exchange8:ME |
| Tenant-authored detection rule modules | Sentin8:Func (execution host) |
| Identity, tokens, M2M client registration | Sentin8:Auth |
| Message delivery between services | Sentin8:Bus |
| Case assignment workflow UI chrome | Platform admin shell (rendering host) |

## 3. Business Capabilities

### Detection patterns

| Capability | Business rule |
|---|---|
| Pattern vocabulary | Exactly eight patterns exist — wash trade, spoofing, layering, momentum ignition, insider-like trading, front-running, marking the close, excessive self-crossing — and each persists under its stable key |
| Severity ranking | Insider-like trading and front-running rank highest (4); the spoofing family ranks 3; close-window and self-crossing rank 2 — severity is fixed per pattern class, not per alert |
| Wash trade | Counts self-crosses (same beneficial owner on both sides) per account and instrument; a single accidental cross never alerts; the alert fires exactly once when the configured repetition threshold is reached |
| Spoofing | Tracks orders cancelled before execution against the account's executed orders in the window; alerts only when the volume floor is met AND the cancel ratio reaches the configured share; confidence derives from the ratio itself |
| Marking the close | Flags trades inside the close window whose notional exceeds a configured multiple of the session's median trade; trades missing a median basis never alert |
| Evidence | Every alert carries its supporting observations, making the case file self-contained — no re-derivation from raw events |

### Case management

| Capability | Business rule |
|---|---|
| One alert, one case | Every detection opens exactly one case; duplicate deliveries of the same detection (same pattern, instrument, account, day) do not create a second case |
| Workflow states | A case moves open → investigating → closed; the closure records whether it was dismissed |
| Case identity | Case identifiers are strictly increasing — no identifier is ever re-used |
| Triage | The open-case view sorts by descending pattern severity; same-severity cases keep open order; closed cases leave the queue |
| Assignment | A case can be assigned to a named reviewer during the workflow |

### Operational alerts

| Capability | Business rule |
|---|---|
| Alert log | Operational messages are recorded with their source; unacknowledged alerts are those without an administrator |
| Acknowledgement | Acknowledging records who and when; the unacked view is index-backed so the sweep stays fast |

## 4. Business Events (Outbound)

| Event | When it fires |
|---|---|
| Case/alert state changes (relayed via the outbox producer) | Every detection and case transition is committed together with an outbox row delivered to Sentin8:Bus |
| Audit journal entries | Every state change is journaled and pushed to the platform audit ingest (`sentin8:sys:audit:write`) |

**Inbound (consumed)**

| Event / input | Source |
|---|---|
| Trade and order lifecycle events from the matching/clearing tier | Exchange8:ME / Exchange8:Clearing / Exchange8:Gateway / Exchange8:OMS (via Sentin8:Bus) |
| Tenant-authored detection rules executed over observations | Sentin8:Func |
| Policy-set updates (scope-mirror rules for this service) | Shield (Sentin8:Policy) hot-reload |
| Session median statistics for close-window detection | market-statistics producers |

## 5. Business Rules & Invariants

1. A detection always carries its evidence: the stored alert and the case
   that references it contain the supporting observations, never just a
   verdict.
2. One detection, one case — duplicate deliveries in the same day-bucket
   (pattern, instrument, account) collapse to a single case.
3. Built-in detectors require the abuse signature: a single self-cross
   is never a wash-trade alert; a balanced order/cancel mix is never a
   spoofing alert; a missing or zero median disables close-window alerts
   rather than mis-flagging.
4. Triage ordering is deterministic: severity descending, ties stable in
   open order, closed cases excluded.
5. Case identifiers never repeat or reverse; a closed case is terminal in
   the workflow (its dismissed flag is the recorded disposition).
6. Every write's durable event record is committed in the same
   transaction as the write; the platform can always reconstruct what
   happened and what must be told to the bus.
7. Authorization is fail-closed: with no policy set loaded nothing passes;
   the token type (user or M2M) never bypasses a policy decision.
8. All alert data is tenant-isolated end to end (row-level security +
   explicit tenant predicates); no service role can bypass it.
9. Operational alerts keep an acknowledgement trail — who acknowledged
   and when — and remain searchable until acknowledged.

## 6. Target-User Roles & Use Cases

| Persona | Role | What they do with the service |
|---|---|---|
| Compliance officer | Primary investigator | Reviews the severity-ranked open-case queue, works cases through investigation, records the disposition (dismissed or substantiated) |
| Compliance supervisor | Team lead | Assigns cases to officers, monitors the unacknowledged ops-alert log, verifies closure quality |
| Regulatory reporting analyst | Downstream consumer | Takes closed cases forward into regulatory reports (produced by Exchange8:Regulatory) |
| On-call operations engineer | Ops consumer | Acknowledges operational alerts so the unacked queue stays empty |
| Platform administrator | Admin | Manages the Shield policy sets and the service's admin pages |
| Venue management | Oversight | Demonstrates to regulators that surveilled patterns and dispositions exist for every alert |

## 7. KPIs / Success Criteria

- **Detection completeness**: 100% of executed trades and order events feed
  the observation stream; no alert is dropped after a platform outage (outbox
  replay is idempotent).
- **Alert precision**: built-in detectors alert only on the pattern's abuse
  signature — zero alerts for single accidental crosses and balanced
  order/cancel profiles.
- **Triage discipline**: the queue always surfaces the most severe open
  case first; no open case is invisible to the queue.
- **Dispositions recorded**: 100% of closed cases carry their dismissed
  flag; no case disappears from the record.
- **Auditability**: every case transition is journaled and pushed; a Sys
  outage never loses an audit row.
- **Security posture**: all authorization decisions pass the enforced
  scope-mirror policy set; tenant isolation proven by the RLS matrix.

## 8. Dependencies & Reliance on Other Services

| Dependency | What the business relies on it for |
|---|---|
| Exchange8:ME / Exchange8:Clearing / Exchange8:Gateway / Exchange8:OMS | The trade and order events that make detection possible |
| Sentin8:Bus | Delivery of the event stream in, and the durable relaying of case/alert events out |
| Sentin8:Func | Execution host for tenant-authored detection rules |
| Exchange8:Regulatory | The regulatory-reporting outlet that consumes case outcomes |
| Shield (Sentin8:Policy) | The enforced authorization decision for every action |
| Sentin8:Auth | Identity plane, M2M client credentials (certificate-bound), token verification |
| Sentin8:Sys | Audit intake (`POST /v1/audit`) and instance resolution |
| Market-statistics producers | Session median notional per instrument for close-window detection |

## 9. Acceptance Criteria

1. Watching a stream of ordinary trades produces no alerts; a single
   self-crossing execution never fires the wash-trade detector.
2. The wash-trade detector fires exactly once when the repetition threshold
   is reached, and different accounts (or instruments) never share a
   counter.
3. A spoofing alert fires only at or above the configured order-volume
   floor AND the configured cancel ratio; its confidence equals the ratio
   (bounded to 100) and its evidence carries the whole cancel burst.
4. A marking-the-close alert fires only for trades inside the window
   (bounds inclusive) whose notional exceeds the configured multiple of the
   session median; without a median no alert is possible.
5. Each detection opens one case; a re-delivery of the same detection in the
   same day-bucket opens nothing more.
6. A case transitions open → investigating → closed (or straight to closed),
   and every transition is journaled; the closure records its dismissed
   flag.
7. The triage view lists open cases by descending pattern severity with
   stable order on ties, and drops closed cases.
8. Operational alerts remain visible unacknowledged until someone
   acknowledges them, and the acknowledgement records who and when.
9. With no policy set loaded, every action is denied (fail-closed); with the
   declared scope-mirror loaded, exactly the declared scopes and platform
   admin scopes pass, M2M and user tokens are treated identically, and a
   tenant-authored forbid overrides the mirror.
10. Every detection's alert row and case row are tenant-isolated: an
    out-of-environment context sees none of them.

## 10. Open Business Questions

- The compliance-desk workflow is designed but its admin data/UI handlers
  are not mounted yet — the RFW `alerts` and `cases` pages exist as
  registrations only.
- The observation wiring from the exchange's trade/order events to detector
  input (consumer-group topology and the duplicate-alert day-bucket dedupe
  it must implement) is designed but not yet connected.
- The remaining built-in detectors (layering, momentum ignition,
  insider-like trading, front-running, excessive self-crossing) currently
  land via tenant-authored rules — which of them should the venue ship as
  built-in baselines is a rule-library decision.
- Where the acknowledged operational-alert producers write from (which
  surfaces create `surveillance.alert` rows) and the ack flow's admin
  surface are not yet implemented beyond the table.
- Case re-open semantics after a dismissal (a follow-up detection on the
  same pattern/instrument/account) and the alert-to-case dedupe window
  length are compliance-process decisions not yet modeled.
- Regulatory reporting consumes closed cases via Exchange8:Regulatory; the exact
  case-outcome payload the reporting service requires is a cross-service
  contract to be finalized there.

---

**Design attribution:** exchange8:surveillance and its underlying concepts were designed by Hamid Jahanzad <Hamid@Jahanzad.com>.
