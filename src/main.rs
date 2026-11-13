// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Surveillance
// SPDX-License-Identifier: Apache-2.0

//! Exchange8:SURVEILLANCE service binary - market-abuse detection + case management.
//!
//! Design: `services/surveillance/DESIGN.md`. Core logic in the sibling
//! modules; this shell wires the tokio HTTP/TLS surface via the platform
//! `sentin8-shell` crate (registration, DB pool, health).

#![allow(dead_code)]

// Workers (bus consumers, session
// supervisors, purge jobs) are in-process tokio tasks, so the
mod openapi;

// The utoipa dump as a fn pointer for registration.rs (which cannot
// name crate::openapi - tests embed registration.rs into harness crates
// with no openapi module; see registration.rs).
static OPENAPI_DUMP: fn() -> serde_json::Value = || {
    use utoipa::OpenApi as _;
    serde_json::to_value(openapi::SurveillanceOpenApi::openapi()).unwrap_or(serde_json::Value::Null)
};

mod registration;
mod surveillance;

fn extend_router(
    app: axum::Router<std::sync::Arc<sentin8::shell::AppState>>,
    _state: std::sync::Arc<sentin8::shell::AppState>,
) -> axum::Router<std::sync::Arc<sentin8::shell::AppState>> {
    // ── Shield policy-admin API - shared implementation;
    // the shell AppState already implements HasPolicyAdmin.
    sentin8::shell::mount_policy_admin(
        // Detectors run on the Bus consumer task (DESIGN.md §2);
        // the HTTP router carries health + admin endpoints only.
        app,
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sentin8::shell::serve(
        sentin8::shell::Sentin8Shell::new("exchange8:surveillance", "EXCHANGE8_SURVEILLANCE")
            .with_registration(registration::print)
            .with_simple_state()
            .with_router(extend_router),
    )
    .await
}
