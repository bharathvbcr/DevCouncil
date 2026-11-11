//! An attribute macro on the function that holds the call rewrites its body: here
//! it adds `use crate::p_db::Dup;` as the first statement. See `c46`.
use crate::p_da::Dup;

#[pm::body_use(crate::p_db::Dup)]
pub fn attr_fn() -> u8 {
    Dup::mk16()
}
