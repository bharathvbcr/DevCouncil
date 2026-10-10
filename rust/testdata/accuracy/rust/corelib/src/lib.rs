//! DevMap Rust construct fixture. Every construct is labelled `Cnn`; the
//! expected symbols and edges are listed in ../../expected.tsv.

pub mod shapes; // C19 mod.rs layout: shapes/mod.rs
pub mod geo; // C19 2018 layout: geo.rs + geo/point.rs
pub mod util;
pub mod traits;
pub mod asyncs;
pub mod macros;
pub mod ops;
pub mod ffi;
pub mod deadprobe;
pub mod probes2;
pub mod guards;
pub mod visibility; // C52 restricted visibility

// C18 re-export chains
pub use shapes::circle::reexported_c18; // plain pub use
pub use util::renamed_src_c18b as renamed_c18b; // renamed pub use
pub use geo::*; // glob re-export (exposes `point`)

use std::cell::RefCell;
use std::fmt::Display;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------- C05 generic impl
pub struct Wrapper<T> {
    inner: T,
}

impl<T> Wrapper<T> {
    pub fn new_c05(inner: T) -> Self {
        Wrapper { inner }
    }
    pub fn get_c05(&self) -> &T {
        &self.inner
    }
}

pub fn c05_caller() -> i32 {
    let w = Wrapper::new_c05(3);
    *w.get_c05()
}

// ---------------------------------------------------------------- C06 associated fn vs method, UFCS
#[derive(Default)]
pub struct Counter {
    n: u32,
}

impl Counter {
    pub fn new_c06() -> Self {
        Counter { n: 0 }
    }
    pub fn incr_c06(&mut self) {
        self.n += 1;
    }
}

pub fn c06_caller() -> u32 {
    let mut c = Counter::new_c06();
    c.incr_c06();
    Counter::incr_c06(&mut c); // UFCS
    c.n
}

// ---------------------------------------------------------------- C08 Deref / auto-ref
pub struct Inner;
impl Inner {
    pub fn inner_c08(&self) -> u8 {
        8
    }
}
pub struct Guard(Inner);
impl Deref for Guard {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

pub fn c08_caller(g: Guard) -> u8 {
    let a = g.inner_c08(); // through Deref
    let r = &&g;
    a + r.inner_c08() // auto-deref through references
}

// ---------------------------------------------------------------- C09 closures / fn items as args
pub fn apply_c09<F: Fn(i32) -> i32>(f: F) -> i32 {
    f(1)
}
pub fn helper_c09(x: i32) -> i32 {
    x + 1
}
pub fn helper_c09b(x: &i32) -> i32 {
    *x * 2
}

pub fn c09_caller() -> i32 {
    let a = apply_c09(|x| helper_c09(x)); // call inside closure
    let v = vec![1, 2, 3];
    let b: i32 = v.iter().map(helper_c09b).sum(); // fn item passed by name
    a + b
}

// ---------------------------------------------------------------- C10 iterator adapter chains
pub struct Item {
    pub v: i32,
}
impl Item {
    pub fn score_c10(&self) -> i32 {
        self.v
    }
    pub fn score_c10b(&self) -> i32 {
        self.v * 2
    }
    pub fn keep_c10(&self) -> bool {
        self.v > 0
    }
}

pub fn c10_caller(items: &[Item]) -> i32 {
    let a: i32 = items.iter().filter(|i| i.keep_c10()).map(|i| i.score_c10()).sum();
    let b: i32 = items.iter().map(Item::score_c10b).sum(); // path reference
    a + b
}

// ---------------------------------------------------------------- C11 ?-propagated calls
pub struct Config {
    pub n: u32,
}
impl Config {
    pub fn validate_c11(&self) -> Result<u32, String> {
        Ok(self.n)
    }
}
pub fn parse_c11(s: &str) -> Result<Config, String> {
    s.parse::<u32>().map(|n| Config { n }).map_err(|e| e.to_string())
}

pub fn c11_caller(s: &str) -> Result<u32, String> {
    let cfg = parse_c11(s)?;
    let n = cfg.validate_c11()?;
    let m = parse_c11(s)?.validate_c11()?; // chained on ?
    Ok(n + m)
}

// ---------------------------------------------------------------- C15 derive / clone false-edge probe
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Derived {
    pub a: u8,
}

pub struct Manual {
    pub a: u8,
}
impl Clone for Manual {
    fn clone(&self) -> Self {
        Manual { a: self.a }
    }
}

pub fn c15_caller(d: &Derived, m: &Manual) -> u8 {
    let d2 = d.clone(); // derived: must NOT bind to Manual::clone
    let m2 = m.clone(); // manual impl: Manual::clone
    let d3 = Derived::default(); // derived: no user symbol
    d2.a + m2.a + d3.a
}

// ---------------------------------------------------------------- C20 cfg / feature gated
#[cfg(feature = "fast")]
pub fn algo_c20() -> u8 {
    1
}
#[cfg(not(feature = "fast"))]
pub fn algo_c20() -> u8 {
    2
}

#[cfg(feature = "fast")]
mod fast_impl {
    pub fn gated_c20b() -> u8 {
        3
    }
}

pub fn c20_caller() -> u8 {
    #[cfg(feature = "fast")]
    let x = fast_impl::gated_c20b();
    #[cfg(not(feature = "fast"))]
    let x = 0;
    algo_c20() + x
}

// ---------------------------------------------------------------- C21 #[test] entry points
pub fn target_c21() -> u8 {
    21
}
pub fn only_tested_c21b() -> u8 {
    22
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_c21() {
        assert_eq!(target_c21(), 21);
    }

    #[tokio::test]
    async fn t2_c21() {
        assert_eq!(only_tested_c21b(), 22);
    }
}

// ---------------------------------------------------------------- C24 const / static
pub const LIMIT_C24: usize = 24;
pub static REG_C24: OnceLock<Vec<u8>> = OnceLock::new();
pub fn init_c24() -> Vec<u8> {
    vec![2, 4]
}
pub fn helper_c24() -> u8 {
    24
}
pub const FPTR_C24: fn() -> u8 = helper_c24; // fn item in a const initializer

pub fn c24_caller() -> usize {
    let r = REG_C24.get_or_init(init_c24); // fn item passed to a std method
    r.len() + LIMIT_C24 + FPTR_C24() as usize
}

// ---------------------------------------------------------------- C25 enums with impl
pub enum Shape {
    Circle(f64),
    Square(f64),
}
impl Shape {
    pub fn new_c25() -> Self {
        Shape::Circle(1.0) // tuple-variant constructor, not a function call
    }
    pub fn area_c25(&self) -> f64 {
        match self {
            Shape::Circle(r) => r * r,
            Shape::Square(s) => s * s,
        }
    }
}

pub fn c25_caller() -> f64 {
    let s = Shape::new_c25();
    s.area_c25() + Shape::Square(2.0).area_c25()
}

// ---------------------------------------------------------------- C28 Self:: and self. inside impl
pub struct Engine;
impl Engine {
    fn helper_c28() -> u8 {
        28
    }
    fn other_c28(&self) -> u8 {
        Self::helper_c28()
    }
    pub fn run_c28(&self) -> u8 {
        self.other_c28()
    }
}

// ---------------------------------------------------------------- C29 call through a fn-pointer field
pub struct Hooks {
    pub cb_c29: Box<dyn Fn(u8) -> u8>,
}
pub fn hook_target_c29(x: u8) -> u8 {
    x
}
pub fn c29_caller() -> u8 {
    let h = Hooks { cb_c29: Box::new(hook_target_c29) };
    (h.cb_c29)(1)
}

// ---------------------------------------------------------------- C30 method on a typed struct field
pub struct Store;
impl Store {
    pub fn save_c30(&self) -> u8 {
        30
    }
}
pub struct Service {
    store: Store,
}
impl Service {
    pub fn c30_caller(&self) -> u8 {
        self.store.save_c30()
    }
}

// ---------------------------------------------------------------- C34 method on a &T parameter
pub fn c34_caller(x: &Store) -> u8 {
    x.save_c30()
}

// ---------------------------------------------------------------- C32 turbofish
pub fn parse_as_c32<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.parse::<T>().ok()
}
pub fn c32_caller() -> Option<u32> {
    let w = Wrapper::<u8>::new_c05(1);
    let _ = w.get_c05();
    parse_as_c32::<u32>("1")
}

// ---------------------------------------------------------------- C33 nested fn item
pub fn outer_c33() -> u8 {
    fn inner_c33() -> u8 {
        33
    }
    inner_c33()
}

// ---------------------------------------------------------------- C36 Rc<RefCell<T>> / Arc<Mutex<T>>
pub struct Bag {
    pub items: Vec<u8>,
}
impl Bag {
    pub fn push_c36(&mut self, x: u8) {
        self.items.push(x);
    }
}
pub fn c36_caller() -> usize {
    let rc = Rc::new(RefCell::new(Bag { items: vec![] }));
    rc.borrow_mut().push_c36(1);
    let m = Arc::new(Mutex::new(Bag { items: vec![] }));
    m.lock().unwrap().push_c36(2);
    let g = m.lock().unwrap();
    let n = rc.borrow().items.len() + g.items.len();
    n
}

// ---------------------------------------------------------------- C37 pattern-bound receivers
pub fn c37_caller(opt: Option<Item>, v: Vec<Item>) -> i32 {
    let mut t = 0;
    if let Some(it) = opt {
        t += it.score_c10();
    }
    for it in &v {
        t += it.score_c10b();
    }
    t
}

// ---------------------------------------------------------------- C04 blanket impl
pub trait Describe {
    fn describe_c04(&self) -> String;
}
impl<T: Display> Describe for T {
    fn describe_c04(&self) -> String {
        format!("<{}>", self)
    }
}
pub fn c04_caller() -> String {
    5.describe_c04()
}
