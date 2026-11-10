//! Run-time checks for the cells whose callee a value distinguishes.
#[cfg(test)]
mod tests {
    #[test]
    fn a_macro_made_use_picks_the_db_type() {
        assert_eq!(crate::c33::mac_block(), 1);
        assert_eq!(crate::c33::mac_block2_value(), 1);
    }

    #[test]
    fn a_block_glob_beats_the_module_item() {
        assert_eq!(crate::c34::macro_glob(), 1);
        assert_eq!(crate::c34::std_glob(), 2);
        assert_eq!(crate::c34::std_glob2(), 5);
        assert_eq!(crate::c35::ext_glob(), 2);
        assert_eq!(crate::c35::macro_item_glob(), 1);
        assert_eq!(crate::c35::plain_via_glob(), 3);
    }

    #[test]
    fn a_statement_macro_can_declare_the_name() {
        assert_eq!(crate::c37::unknown_macro_then_name(), 1);
        assert_eq!(crate::c38::shadowed_dbg(), 1);
        assert_eq!(crate::c37::std_macro_then_outer(), 2);
    }
}
