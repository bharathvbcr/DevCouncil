//! The two classes the Rust recall floors are set on: `free_self` and
//! `non_self`. The class is a function of how the site is written and whether
//! the target is a method, never of what DevMap resolved.

use rustlabel::classify::{classify, Construct, SiteShape};

fn path(qualifier: &str) -> SiteShape {
    SiteShape::PathCall {
        qualifier: qualifier.into(),
    }
}

fn method(receiver: &str) -> SiteShape {
    SiteShape::MethodCall {
        receiver: receiver.into(),
    }
}

#[test]
fn a_bare_call_is_free_self() {
    assert_eq!(classify(&SiteShape::BareCall, false), Construct::FreeSelf);
}

#[test]
fn a_method_call_on_self_is_free_self_and_on_anything_else_is_non_self() {
    assert_eq!(classify(&method("self"), true), Construct::FreeSelf);
    assert_eq!(classify(&method("other"), true), Construct::NonSelf);
    // `self.store.get()` is a call on a field, not on `self`.
    assert_eq!(classify(&method("self.store"), true), Construct::NonSelf);
    assert_eq!(classify(&method(" self "), true), Construct::FreeSelf);
}

#[test]
fn a_path_call_is_free_unless_it_names_a_method_of_a_type_other_than_self() {
    assert_eq!(classify(&path("Self"), true), Construct::FreeSelf);
    assert_eq!(classify(&path("self"), false), Construct::FreeSelf);
    assert_eq!(classify(&path("util"), false), Construct::FreeSelf);
    assert_eq!(classify(&path("crate::util"), false), Construct::FreeSelf);
    assert_eq!(classify(&path("Counter"), true), Construct::NonSelf);
    // A trait path call: `Speak::speak(&d)` reaches a method.
    assert_eq!(classify(&path("Speak"), true), Construct::NonSelf);
    assert_eq!(classify(&path("<T as Speak>"), true), Construct::NonSelf);
}

#[test]
fn an_operator_is_non_self() {
    assert_eq!(classify(&SiteShape::Operator, true), Construct::NonSelf);
}

#[test]
fn a_function_used_as_a_value_follows_the_path_rule() {
    let value = |qualifier: Option<&str>| SiteShape::ValuePath {
        qualifier: qualifier.map(str::to_string),
    };
    assert_eq!(classify(&value(None), false), Construct::FreeSelf);
    assert_eq!(classify(&value(Some("Self")), true), Construct::FreeSelf);
    assert_eq!(classify(&value(Some("Counter")), true), Construct::NonSelf);
    assert_eq!(classify(&value(Some("util")), false), Construct::FreeSelf);
}

#[test]
fn the_labels_file_spelling_is_the_one_the_scorer_keys_on() {
    assert_eq!(Construct::FreeSelf.as_str(), "free_self");
    assert_eq!(Construct::NonSelf.as_str(), "non_self");
}
