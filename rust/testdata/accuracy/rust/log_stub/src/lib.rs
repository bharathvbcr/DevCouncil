//! A stand-in for `log::debug!`/`log::trace!`: expression macros that declare nothing.
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => {};
}

#[macro_export]
macro_rules! trace {
    ($($t:tt)*) => {};
}
