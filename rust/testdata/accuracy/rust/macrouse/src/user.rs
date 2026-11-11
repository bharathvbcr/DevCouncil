use crate::mda::Dup;

pub fn via_macro_use() -> u8 {
    vec!(crate::mdb::Dup);
    Dup::mk16()
}
