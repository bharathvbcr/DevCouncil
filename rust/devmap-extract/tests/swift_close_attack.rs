//! Closing adversarial probes for 30a5eade, extraction side. A failing test is
//! a BREAK.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind};

fn receivers(extraction: &Extraction, name: &str) -> Vec<Option<String>> {
    extraction
        .references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Name && r.name == name)
        .map(|r| r.receiver_expr.clone())
        .collect()
}

// ------------------------------------------------------------ #7 infix operands

#[test]
fn wrong_a_right_operand_member_keeps_its_own_receiver_for_every_operator() {
    let operators = [
        "+", "-", "*", "/", "%", "??", "==", "!=", "<", ">", "<=", "&&", "||", "...", "..<", "<<",
        "&+", "&-", "<+>", "**", "|>", "===", "~=", "^", "&", "|",
    ];
    let mut bad = Vec::new();
    for op in operators {
        let src = format!("func f(_ p: P, _ q: Q) {{ _ = p.aa {op} q.bb }}\n");
        let ex = extract_file("S.swift", &src);
        let a = receivers(&ex, "aa");
        let b = receivers(&ex, "bb");
        if a != vec![Some("p".to_string())] || b != vec![Some("q".to_string())] {
            bad.push(format!("{op}: aa={a:?} bb={b:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
#[ignore = "pre-existing operator receiver text, ft-915b0a03"]
fn wrong_a_three_operand_chain_never_makes_a_binary_expression_a_receiver() {
    let cases = [
        ("p.a + q.b.c", "c"),
        ("p.a + q.b + r.c", "c"),
        ("p.a * q.b + r.c.d", "d"),
        ("p.a ?? q.b.c", "c"),
        ("p.a == q.b.c", "c"),
        ("!p.q.r", "r"),
        ("-p.q.r", "r"),
        ("p.a + (q.b).c", "c"),
        ("(p.a + q.b).c", "c"),
        ("p.a + q().b", "b"),
        ("p.a + q[0].b", "b"),
        ("p.a + q?.b", "b"),
        ("p.a + q!.b", "b"),
        ("p.a + q.b()", "b"),
    ];
    let mut bad = Vec::new();
    for (expr, tail) in cases {
        let src = format!("func f(_ p: P, _ q: Q, _ r: R) {{ _ = {expr} }}\n");
        let ex = extract_file("S.swift", &src);
        for receiver in receivers(&ex, tail).into_iter().flatten() {
            if receiver.contains(" + ")
                || receiver.contains(" ?? ")
                || receiver.contains(" == ")
                || receiver.contains(" * ")
                || receiver.starts_with('!')
                || receiver.starts_with('-')
                || receiver.starts_with("p.a")
            {
                bad.push(format!("{expr}: {tail} receiver {receiver:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
#[ignore = "pre-existing operator receiver text, ft-915b0a03"]
fn wrong_a_prefix_operator_does_not_enter_the_receiver() {
    let ex = extract_file("S.swift", "func f(_ p: P) { _ = !p.q; _ = -p.n; _ = ~p.m }\n");
    assert_eq!(receivers(&ex, "q"), vec![Some("p".to_string())]);
    assert_eq!(receivers(&ex, "n"), vec![Some("p".to_string())]);
    assert_eq!(receivers(&ex, "m"), vec![Some("p".to_string())]);
}

// ------------------------------------------------------------ #6 leading-dot

fn leading(src: &str, name: &str) -> Vec<Option<String>> {
    receivers(&extract_file("S.swift", src), name)
}

#[test]
fn wrong_a_leading_dot_under_a_closure_of_another_type_does_not_take_the_functions_type() {
    let got = leading(
        "enum Shape { case circle }\nenum Other { case circle }\nfunc f() -> Shape { let g: () -> Other = { return .circle }; _ = g; return .circle }\n",
        "circle",
    );
    // one leading-dot (the closure's) must not be `Shape`; the other may be.
    assert!(
        got.iter().filter(|r| **r == Some("Shape".to_string())).count() <= 1,
        "{got:?}"
    );
}

#[test]
fn wrong_an_implicit_return_closure_in_a_typed_function_names_no_owner() {
    let got = leading(
        "enum Shape { case circle }\nfunc f() -> Shape { let g: () -> Int = { .circle }; _ = g; return Shape.make() }\n",
        "circle",
    );
    assert!(!got.contains(&Some("Shape".to_string())), "{got:?}");
}

#[test]
fn wrong_a_nested_function_returning_another_type_takes_its_own_return_type() {
    let got = leading(
        "enum Shape { case circle }\nenum Other { case circle }\nfunc f() -> Shape { func inner() -> Other { .circle }; _ = inner(); return .circle }\n",
        "circle",
    );
    let mut got = got;
    got.sort();
    assert_eq!(got, vec![Some("Other".to_string()), Some("Shape".to_string())]);
}

#[test]
fn wrong_a_ternary_or_switch_leading_dot_names_no_wrong_owner() {
    let got = leading(
        "enum Shape { case a, b }\nenum Other { case a, b }\nfunc f(_ c: Bool) -> Other { let s: Shape = c ? .a : .b; _ = s; return .a }\n",
        "a",
    );
    // `.a` in the ternary is Shape's; the function's stated type is Other.
    assert!(!got.contains(&Some("Other".to_string())) || got.len() == 1, "{got:?}");
}

#[test]
fn wrong_a_switch_returning_leading_dot_names_no_wrong_owner() {
    let got = leading(
        "enum Shape { case a }\nfunc f(_ n: Int) -> Shape { switch n { case 1: return .a; default: return .a } }\n",
        "a",
    );
    for receiver in &got {
        assert!(receiver.is_none() || receiver == &Some("Shape".to_string()), "{got:?}");
    }
}

#[test]
#[ignore = "leading-dot contextual type through an optional, ft-915b0a03"]
fn recall_a_leading_dot_returned_through_an_optional_result_binds_the_wrapped_type() {
    let got = leading("enum Shape { case circle }\nfunc f() -> Shape? { .circle }\n", "circle");
    assert_eq!(got, vec![Some("Shape".to_string())], "{got:?}");
}

#[test]
#[ignore = "leading-dot contextual type through Self, ft-915b0a03"]
fn recall_a_leading_dot_through_a_self_result_binds_the_owner() {
    let got = leading(
        "struct S { static var a = S()\n static func make() -> Self { .a } }\n",
        "a",
    );
    assert_eq!(got, vec![Some("S".to_string())], "{got:?}");
}

#[test]
fn wrong_a_generic_result_names_no_concrete_owner() {
    let got = leading(
        "struct T { static var a = 1 }\nfunc f<T: P>() -> T { .a }\n",
        "a",
    );
    // `T` here is the generic parameter, not the struct `T`.
    assert!(got.iter().all(|r| r.is_none()), "{got:?}");
}

#[test]
fn wrong_a_leading_dot_in_a_default_argument_or_assignment_names_no_function_owner() {
    let got = leading(
        "enum Shape { case a }\nenum Other { case a }\nfunc f(_ o: Other = .a) -> Shape { return Shape.a }\n",
        "a",
    );
    assert!(!got.contains(&Some("Shape".to_string())) || got.len() > 1, "{got:?}");
}

#[test]
fn wrong_a_chained_leading_dot_names_no_owner_for_the_member() {
    let got2 = leading("enum Shape { case circle }\nfunc g() -> Shape { .circle.next }\n", "circle");
    assert!(got2.iter().all(|r| r.is_none()), "{got2:?}");
}

// ------------------------------------------------------------ #8 key paths

#[test]
fn wrong_a_keypath_root_less_member_has_no_receiver() {
    let ex = extract_file("S.swift", "func f(_ xs: [S]) { _ = xs.map(\\.secret) }\n");
    assert!(receivers(&ex, "secret").iter().all(|r| r.is_none()), "{:?}", receivers(&ex, "secret"));
}

#[test]
fn wrong_a_chained_keypath_tail_is_not_given_the_root_as_receiver() {
    let ex = extract_file("S.swift", "func f() { _ = \\S.secret.count }\n");
    assert_eq!(receivers(&ex, "secret"), vec![Some("S".to_string())]);
    assert!(
        receivers(&ex, "count").iter().all(|r| r.as_deref() != Some("S")),
        "{:?}",
        receivers(&ex, "count")
    );
}

#[test]
fn wrong_an_array_root_keypath_is_not_given_the_element_as_receiver() {
    let ex = extract_file("S.swift", "func f() { _ = \\[S].first }\n");
    assert!(
        receivers(&ex, "first").iter().all(|r| r.as_deref() != Some("S")),
        "{:?}",
        receivers(&ex, "first")
    );
}

#[test]
fn wrong_an_optional_root_keypath_is_not_given_a_wrong_receiver() {
    let ex = extract_file("S.swift", "func f() { _ = \\S?.secret; _ = \\Dictionary<String, S>.count }\n");
    for name in ["secret", "count"] {
        assert!(
            receivers(&ex, name).iter().all(|r| r.as_deref() != Some("S") || name == "secret"),
            "{name}: {:?}",
            receivers(&ex, name)
        );
    }
}
