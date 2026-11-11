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

    #[test]
    fn a_dependency_macro_or_attribute_can_rebind_the_type() {
        assert_eq!(crate::c40::imported_info(), 1);
        assert_eq!(crate::c42::aliased_log(), 1);
        assert_eq!(crate::c43::attr_use(), 1);
    }

    #[test]
    fn thread_local_and_bail_declare_no_other_name() {
        assert_eq!(crate::c44::after_thread_local(), 2);
        assert_eq!(crate::c44::after_bail(false), Ok(3));
        assert_eq!(crate::c44::after_bail(true), Err("no".to_string()));
        assert_eq!(crate::c44::after_bail_tail(true), Ok(4));
        assert_eq!(crate::c44::after_bail_tail(false), Err("no".to_string()));
    }

    #[test]
    fn a_glob_of_a_dependency_brings_its_macro_over_the_prelude_and_the_import() {
        assert_eq!(crate::c45::glob_bail(), 1);
        assert_eq!(crate::c45b::prelude_bail(), 1);
    }

    #[test]
    fn an_attribute_macro_on_an_item_or_a_function_rebinds_the_type() {
        assert_eq!(crate::c46::attr_module(), 1);
        assert_eq!(crate::c46b::attr_fn(), 1);
    }

    #[test]
    fn cfg_attr_and_a_pathed_derive_can_emit_a_use() {
        assert_eq!(crate::c47::cfg_attr_use(), 1);
        assert_eq!(crate::c47b::pathed_derive(), 1);
    }

    #[test]
    fn a_glob_of_std_or_of_a_crate_without_the_macro_changes_nothing() {
        assert_eq!(crate::c49::std_glob_silent(), 2);
        assert_eq!(crate::c49b::indexed_glob_silent(), 3);
    }
}
