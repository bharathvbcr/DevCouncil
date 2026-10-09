//! C01 static trait dispatch, C02 dyn, C03 generic bound, C07 impl Trait,
//! C27 trait default methods, C31 extension trait on a foreign type,
//! C35 trait static fn through a type parameter.

pub trait Speak {
    fn speak_c01(&self) -> String;
}
pub struct Dog;
impl Speak for Dog {
    fn speak_c01(&self) -> String {
        "woof".into()
    }
}
pub fn c01_caller(d: &Dog) -> String {
    d.speak_c01()
}

pub trait Render {
    fn render_c02(&self) -> u8;
    fn area_c03(&self) -> u8;
}
pub struct Sq;
pub struct Ci;
impl Render for Sq {
    fn render_c02(&self) -> u8 {
        1
    }
    fn area_c03(&self) -> u8 {
        1
    }
}
impl Render for Ci {
    fn render_c02(&self) -> u8 {
        2
    }
    fn area_c03(&self) -> u8 {
        2
    }
}

pub fn c02_caller(s: &dyn Render) -> u8 {
    s.render_c02()
}
pub fn c02b_caller(v: Vec<Box<dyn Render>>) -> u8 {
    v.iter().map(|r| r.render_c02()).sum()
}

pub fn c03_caller<T: Render>(t: &T) -> u8 {
    t.area_c03()
}

pub fn make_c07() -> impl Render {
    Sq
}
pub fn c07_caller() -> u8 {
    let r = make_c07();
    r.render_c02()
}

pub trait Template {
    fn tmpl_c27(&self) -> u8 {
        self.hook_c27() + 1
    }
    fn hook_c27(&self) -> u8;
}
pub struct Concrete;
impl Template for Concrete {
    fn hook_c27(&self) -> u8 {
        27
    }
}
pub fn c27_caller(c: &Concrete) -> u8 {
    c.tmpl_c27()
}

pub trait StrExt {
    fn ext_c31(&self) -> usize;
}
impl StrExt for str {
    fn ext_c31(&self) -> usize {
        self.len()
    }
}
pub fn c31_caller() -> usize {
    "abc".ext_c31()
}

pub trait Factory {
    fn create_c35() -> Self;
}
impl Factory for Dog {
    fn create_c35() -> Self {
        Dog
    }
}
pub fn c35_caller<T: Factory>() -> T {
    T::create_c35()
}
