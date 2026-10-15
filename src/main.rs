// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Surveillance
// SPDX-License-Identifier: Apache-2.0

//! Exchange8:SURVEILLANCE service binary — market-abuse detection + case management.
//!
//! Design: `services/surveillance/DESIGN.md`. Core logic in the sibling
//! modules; this shell wires the tokio HTTP/TLS surface via the platform
//! `sentin8-shell` crate (registration, DB pool, health).

#![allow(dead_code)]

// Global allocator — mimalloc for every exchange8 service binary.
// Workers (bus consumers, session
// supervisors, purge jobs) are in-process tokio tasks, so the
// binary-level allocator covers them all.
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod registration;
mod surveillance;

fn extend_router(
    app: axum::Router<std::sync::Arc<sentin8::shell::AppState>>,
    _state: std::sync::Arc<sentin8::shell::AppState>,
) -> axum::Router<std::sync::Arc<sentin8::shell::AppState>> {
    // ── Shield policy-admin API — shared implementation;
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
