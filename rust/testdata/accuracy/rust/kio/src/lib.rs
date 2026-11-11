//! Macros that carry the names of std macros and emit a `use` in the scope of the
//! call. Defined under another name and re-exported, as a dependency the index
//! never sees would define them.
#[macro_export]
macro_rules! __kio_use {
    ($($t:tt)+) => {
        use $($t)+;
    };
}

pub use __kio_use as println;
pub use __kio_use as dbg;
pub use __kio_use as vec;
pub use __kio_use as format;
