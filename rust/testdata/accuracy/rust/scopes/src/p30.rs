//! A `#[path]` child inherits the names its parent imports, here through a
//! module that only re-exports.
use crate::p30_re::helper30;

#[path = "p30_kid.rs"]
pub mod kid;
