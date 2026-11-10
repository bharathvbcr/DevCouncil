//! Rust caller-accuracy fixture (perf audit 2026-11-09). One module per cause
//! class F2 (dyn dispatch), F3 (glob-chain bare names), F4 (`#[path]` test
//! modules), F4b (same-file item vs glob namesake), F8 (field named like a
//! method). Expected edges: ../../edges.tsv rows `rs.f*`.

pub mod dynd;
pub mod dyngate;
pub mod gateother;
pub mod fieldname;
pub mod handlers;
pub mod otherq;
pub mod pathmod;
pub mod plainmod;
pub mod runner;
pub mod sameglob;

/// Names `Quarantine` with no declaration and no import in this file: two
/// files declare one, so no edge is the right answer.
pub fn anon_f4() {
    let _ = Quarantine::new_f4();
}
