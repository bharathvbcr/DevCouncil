//! A macro invoked in the block emits a block-level `use`: the module-level import
//! of the same type name is shadowed there.
use crate::p_da::Dup;

macro_rules! use_db {
    () => {
        use crate::p_db::Dup;
    };
}

pub fn mac_block() -> u8 {
    use_db!();
    Dup::mk16()
}

pub fn mac_block2_value() -> u8 {
    let v = {
        use_db!();
        Dup::mk16()
    };
    v
}
