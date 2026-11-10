//! Macros that look like `log::info!` and `vec!` and emit a `use` in the scope of
//! the call. They are defined under other names and re-exported, as a dependency
//! the index never sees would define them: no `macro_rules! info` is in the
//! corpus.
#[macro_export]
macro_rules! __emit_use {
    ($($t:tt)+) => {
        use $($t)+;
    };
}

pub use __emit_use as info;
pub use __emit_use as vec;
