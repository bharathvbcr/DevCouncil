//! Rust caller-accuracy fixture (perf audit 2026-11-09). One module per cause
//! class F2 (dyn dispatch), F3 (glob-chain bare names), F4 (`#[path]` test
//! modules), F4b (same-file item vs glob namesake), F8 (field named like a
//! method). Expected edges: ../../edges.tsv rows `rs.f*`.

pub mod dynd;
pub mod dyngate;
pub mod dyntwin_a;
pub mod dyntwin_b;
pub mod gateother;
pub mod fieldname;
pub mod g1;
pub mod g1da;
pub mod g2;
pub mod g2db;
pub mod g3;
pub mod g3db;
pub mod g4;
pub mod g5;
pub mod g5c;
pub mod g5chain;
pub mod g5m;
pub mod g5mchain;
pub mod g5mod;
pub mod g5v;
pub mod g6err;
pub mod g6src;
pub mod g6two;
pub mod g6use;
pub mod util_g4;
pub mod handlers;
pub mod inlinemod;
pub mod otherq;
pub mod pathmod;
pub mod plainmod;
pub mod runner;
pub mod scopemod;
pub mod sameglob;

/// Names `Quarantine` with no declaration and no import in this file: two
/// files declare one, so no edge is the right answer.
pub fn anon_f4() {
    let _ = Quarantine::new_f4();
}
