//! C17 cross-crate calls, C18 re-export chains, C19 module paths through a crate.

use corelib::shapes::circle::Circle;
use corelib::util;
use corelib::traits::{Render, Speak};

fn c17_caller() -> f64 {
    let a = corelib::util::cross_c17() as f64; // fully qualified cross-crate
    let b = util::cross_c17() as f64; // through `use corelib::util`
    let c = Circle::new_c17(2.0); // imported type, associated fn
    a + b + c.radius_c17() // method on a cross-crate type
}

fn c18_caller() -> u8 {
    let a = corelib::reexported_c18(); // pub use chain
    let b = corelib::renamed_c18b(); // pub use ... as ...
    let c = corelib::point::dist_c19(1.0, 1.0) as u8; // through `pub use geo::*`
    a + b + c
}

fn c01_cross(d: &corelib::traits::Dog) -> String {
    d.speak_c01() // trait method from another crate, trait imported
}

fn c02_cross(r: &dyn Render) -> u8 {
    r.render_c02()
}

#[tokio::main]
async fn main() {
    let _ = c17_caller();
    let _ = c18_caller();
    let _ = c01_cross(&corelib::traits::Dog);
    let _ = c02_cross(&corelib::traits::Sq);
    let _ = corelib::asyncs::c16_caller(&corelib::asyncs::Client).await;
}
