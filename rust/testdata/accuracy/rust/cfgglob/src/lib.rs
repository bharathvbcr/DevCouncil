//! Two cfg alternatives of one module, each a glob import of a module that holds an
//! item-position macro call (so the glob may bring any name) and a type `Inner`.
//! Only the unix one has `unix_only`, so (the crate compiling) the call is that one.
//! The inner attribute is wry's: it belongs to the crate, not to the item after it.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android::*;

#[cfg(unix)]
mod unixy;
#[cfg(unix)]
use unixy::*;

pub fn only_unix() -> u8 {
    Inner::unix_only()
}
