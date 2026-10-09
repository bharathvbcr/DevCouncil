//! Probes added after the corpus sample (expectations written before rebuild).
//! C43 method call on a foreign type vs a same-file free fn of the same name.
//! C45 method name inside macro args vs a same-file free fn of the same name.
//! C46 receiver typed by a free function's declared return type.

use std::process::Command;

/// A free function that shares its name with std's `Command::spawn`.
pub fn spawn(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    cmd.spawn() // C43: std Command::spawn — must NOT bind to this free fn (self-recursion)
}

pub trait SpawnExt {
    fn spawn_ext_c43(&mut self) -> std::io::Result<std::process::Child>;
}
impl SpawnExt for Command {
    fn spawn_ext_c43(&mut self) -> std::io::Result<std::process::Child> {
        self.spawn() // C43: std Command::spawn — must NOT bind to free fn `spawn`
    }
}

pub struct Gauge;
impl Gauge {
    pub fn level_c45(&self) -> u8 {
        45
    }
}
/// Free function sharing the method's name.
pub fn level_c45() -> u8 {
    0
}
pub fn c45_caller(g: &Gauge) {
    assert_eq!(g.level_c45(), 45); // C45: Gauge.level_c45 only; no edge to free level_c45
    println!("{}", g.level_c45());
}

pub struct Outcome;
impl Outcome {
    pub fn read_c46(&self) -> u8 {
        46
    }
}
pub fn make_outcome_c46() -> Outcome {
    Outcome
}
pub fn try_outcome_c46() -> Result<Outcome, String> {
    Ok(Outcome)
}
pub mod sub {
    pub fn nested_outcome_c46() -> super::Outcome {
        super::Outcome
    }
}
pub fn c46_caller() -> Result<u8, String> {
    let o = make_outcome_c46(); // let bound to free fn result
    let a = o.read_c46();
    let b = make_outcome_c46().read_c46(); // chained on free fn result
    let c = try_outcome_c46()?.read_c46(); // chained through ?
    let p = sub::nested_outcome_c46(); // module-qualified free fn
    let d = p.read_c46();
    Ok(a + b + c + d)
}

// C48: a call inside the initializer of a let whose pattern binds the callee's name.
fn shadow_a_c48() -> u8 {
    1
}
fn shadow_b_c48(x: u8) -> Option<u8> {
    Some(x)
}
fn shadow_c_c48() -> u8 {
    3
}
pub fn c48_caller(opt: Option<u8>) -> u8 {
    let shadow_a_c48 = shadow_a_c48(); // simple shadow
    let (n, shadow_b_c48) = (1, opt.and_then(|p| shadow_b_c48(p))); // tuple pattern + closure
    let (m, shadow_c_c48) = (2, shadow_c_c48()); // tuple pattern, no closure
    shadow_a_c48 + n + m + shadow_b_c48.unwrap_or(0) + shadow_c_c48
}
