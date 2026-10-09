//! C12 macro_rules! (definition, invocation, calls inside the body),
//! C13 format!-style and assert!-style macro arguments.

pub fn helper_c12() -> u8 {
    12
}
pub fn emit_c12(m: &str) -> usize {
    m.len()
}

macro_rules! twice_c12 {
    ($e:expr) => {
        $e + $e
    };
}

macro_rules! log_c12 {
    ($m:expr) => {
        $crate::macros::emit_c12($m)
    };
}

pub fn c12_caller() -> usize {
    let a = twice_c12!(helper_c12()); // call in macro argument
    let b = log_c12!("x"); // call lives only in the macro body
    a as usize + b
}

pub struct Named;
impl Named {
    pub fn name_c13(&self) -> String {
        "n".into()
    }
}
pub fn fmt_c13(x: u8) -> String {
    x.to_string()
}
pub fn calc_c13() -> u8 {
    3
}
pub fn c13_caller(n: &Named) -> String {
    println!("{}", fmt_c13(1));
    assert_eq!(calc_c13(), 3);
    let v = vec![fmt_c13(2)];
    format!("{} {}", n.name_c13(), v.len())
}
