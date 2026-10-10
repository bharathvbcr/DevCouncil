//! C52 (added for the dead-policy card, second pass): restricted visibility.
//! `pub(crate)` and `pub(super)` confine an item to its crate, so an uncalled
//! one is a dead-code candidate even though the `pub` keyword is present.

pub(crate) fn crate_private_unused_c52() -> u8 {
    52
}

pub(crate) fn crate_private_used_c52() -> u8 {
    1
}

pub(super) fn super_private_unused_c52() -> u8 {
    53
}

pub fn uses_crate_private_c52() -> u8 {
    crate_private_used_c52()
}
