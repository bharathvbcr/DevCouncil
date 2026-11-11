//! `gm` is made by a macro: the index sees no symbol for it.
macro_rules! mk_gm {
    () => {
        pub fn gm() -> u8 { 1 }
    };
}

mk_gm!();
