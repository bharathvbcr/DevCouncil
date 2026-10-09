//! C19 file inside a mod.rs directory; C17 cross-crate type; C18 re-export source.

pub struct Circle {
    pub r: f64,
}
impl Circle {
    pub fn new_c17(r: f64) -> Self {
        Circle { r }
    }
    pub fn radius_c17(&self) -> f64 {
        self.r
    }
}

pub fn area_c19(r: f64) -> f64 {
    r * r * 3.0
}

pub fn reexported_c18() -> u8 {
    18
}
