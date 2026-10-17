//! rustlabel's tests, one target. `golden` needs rust-analyzer and is
//! `#[ignore]`d with a reason, so a default run reports it as not run.

mod classify;
mod cli;
mod golden;
mod labelling;
mod namer;
mod scan;
