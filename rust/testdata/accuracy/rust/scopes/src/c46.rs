//! Known limit (see `RustAttrMacro` in model.rs): an attribute macro on a
//! module-level item, or on the function that holds the call, can emit a `use`
//! that the resolver does not see, because taking every attribute macro as
//! unknown would leave nearly every function without an edge. Both calls here and
//! in `c46b` mean the second `Dup`; the resolver binds the first.
use crate::p_da::*;

#[pm::with_use(crate::p_db::Dup)]
struct Marker46;

pub fn attr_module() -> u8 {
    let _marker = Marker46;
    Dup::mk16()
}
