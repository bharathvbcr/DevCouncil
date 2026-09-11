//! Phase 7 retired DevCouncil's Python storage layer.
//!
//! This file used to drive both planes against one SQLite file: Rust acquired,
//! Python read back through `TaskLeaseRepository` (sqlmodel), and the reverse.
//! Phase 7 (2026-09-10) deleted that Python package. A DevCouncil checkout can
//! still look "present" (`.venv` + `src/devcouncil` stubs), so the old locator
//! found an interpreter and then panicked on `ModuleNotFoundError: sqlmodel` —
//! which made an intentionally retired surface look like a broken gate.
//!
//! Lease behaviour is covered by the Rust-native suite (`leases.rs` and the
//! unit tests under `src/`). Reintroduce a cross-plane interop check only when
//! a second writer of the same schema exists.

#[test]
fn python_storage_interop_retired_with_phase_7() {
    // Recorded retirement: keep this test so `cargo test -p dc-store` still
    // collects an explicit statement that the Python side is gone, rather than
    // a silent absence that reads like the check was never written.
}
