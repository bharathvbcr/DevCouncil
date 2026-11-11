//! A statement-position macro in the block, then a name an outer scope declares.
pub fn h37a() -> u16 { 2 }

pub fn std_macro_then_outer() -> u16 {
    println!("x");
    let _v = vec![1u8];
    h37a()
}

macro_rules! emit_h37 {
    () => {
        fn h37b() -> u8 { 1 }
    };
}

pub fn h37b() -> u16 { 2 }

pub fn unknown_macro_then_name() -> u8 {
    emit_h37!();
    h37b()
}
