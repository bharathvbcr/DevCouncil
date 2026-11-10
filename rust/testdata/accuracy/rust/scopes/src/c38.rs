//! A crate macro named like a std macro is not the std macro.
macro_rules! dbg {
    ($($t:tt)*) => {
        fn h38() -> u8 { 1 }
    };
}

pub fn h38() -> u16 { 2 }

pub fn shadowed_dbg() -> u8 {
    dbg!(1);
    h38()
}
