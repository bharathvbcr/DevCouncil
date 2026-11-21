#![cfg(feature = "parse")]
//! ft-0e6cc90f criterion 9. `has_external_supertype` keeps a Kotlin class that
//! extends `android.app.Service` from binding a bare `getString(..)` to a repo
//! top-level `getString`. It must not also cost recall:
//!
//! * a Java class that inherits an in-repo member through an in-repo base that
//!   itself extends an external class calls that member bare, and the member is
//!   the receiver's own, found before the global rung is asked.
//!
//! The Kotlin cases are `#[ignore]`d: Kotlin has no such rung until the
//! extraction carries lambda-literal spans (ft-87b28bb6). The Kotlin tests left
//! active assert that nothing wrong is bound meanwhile.
//!
//! Each case pairs the recall with the guard it must not trip over: the
//! same-named namesake elsewhere in the repo never receives the call.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// Every `Calls` edge out of `caller`, as `(target symbol, rung)`.
fn calls_from(result: &ResolutionResult, caller: &str) -> Vec<(String, String)> {
    let mut rows: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == caller)
        .map(|edge| {
            (
                edge.target_symbol.clone(),
                format!("{:?}", edge.evidence.expect("resolver evidence").kind),
            )
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

#[test]
#[ignore = "ft-87b28bb6: needs Kotlin lambda-literal spans (extractor fact)"]
fn a_kotlin_bare_call_reaches_a_member_inherited_through_an_external_root() {
    let result = resolve(&[
        (
            "app/Base.kt",
            "package app\n\nimport android.app.Service\n\nopen class Base : Service() {\n    \
             fun helper(): Int = 1\n}\n",
        ),
        (
            "app/Child.kt",
            "package app\n\nclass Child : Base() {\n    fun go(): Int = helper()\n}\n",
        ),
        ("other/Decoy.kt", "package other\n\nfun helper(): Int = 2\n"),
    ]);
    assert_eq!(
        calls_from(&result, "app/Child.kt::Child.go"),
        vec![(
            "app/Base.kt::Base.helper".to_string(),
            "ReceiverType".to_string()
        )],
        "helper() inside Child is the member Child inherits from Base, whatever Base's own base is"
    );
}

#[test]
fn a_java_bare_call_reaches_a_member_inherited_through_an_external_root() {
    let result = resolve(&[
        (
            "app/Base.java",
            "package app;\n\nimport android.app.Service;\n\npublic class Base extends Service {\n    \
             int helper() { return 1; }\n}\n",
        ),
        (
            "app/Child.java",
            "package app;\n\npublic class Child extends Base {\n    int go() { return helper(); }\n}\n",
        ),
        (
            "other/Decoy.java",
            "package other;\n\npublic class Decoy {\n    static int helper() { return 2; }\n}\n",
        ),
    ]);
    assert_eq!(
        calls_from(&result, "app/Child.java::Child.go"),
        vec![(
            "app/Base.java::Base.helper".to_string(),
            "ReceiverType".to_string()
        )],
        "helper() inside Child is the member Child inherits from Base"
    );
}

#[test]
fn a_java_class_implementing_an_external_interface_still_reaches_its_inherited_member() {
    let result = resolve(&[
        (
            "app/Base.java",
            "package app;\n\npublic class Base {\n    int helper() { return 1; }\n}\n",
        ),
        (
            "app/Worker.java",
            "package app;\n\npublic class Worker extends Base implements Runnable {\n    \
             public void run() { helper(); }\n}\n",
        ),
        (
            "other/Decoy.java",
            "package other;\n\npublic class Decoy {\n    static int helper() { return 2; }\n}\n",
        ),
    ]);
    assert_eq!(
        calls_from(&result, "app/Worker.java::Worker.run"),
        vec![(
            "app/Base.java::Base.helper".to_string(),
            "ReceiverType".to_string()
        )],
        "implementing Runnable does not make the member Worker inherits from Base an unknown"
    );
}

#[test]
fn a_kotlin_inherited_sdk_member_still_binds_to_no_repo_namesake() {
    let result = resolve(&[
        (
            "app/Svc.kt",
            "package app\n\nimport android.app.Service\n\nclass PollService : Service() {\n    \
             fun label(): String = getString(1)\n}\n",
        ),
        (
            "util/Strings.kt",
            "package util\n\nfun getString(i: Int): String = i.toString()\n",
        ),
    ]);
    assert!(
        calls_from(&result, "app/Svc.kt::PollService.label").is_empty(),
        "getString is android.app.Service's, not util.getString: {:?}",
        calls_from(&result, "app/Svc.kt::PollService.label")
    );
}

// ---- Attack round: the rung must not promote or add a wrong edge. ----------
//
// Where the rung cannot prove the member, it abstains and the call falls to the
// rungs after it, exactly as before the rung existed. So each case asserts that
// no `ReceiverType` edge leaves the caller, not what the later rungs then say.

fn no_receiver_type_edge(result: &ResolutionResult, caller: &str, why: &str) {
    let rows = calls_from(result, caller);
    assert!(
        rows.iter().all(|(_, rung)| rung != "ReceiverType"),
        "{why}: {rows:?}"
    );
}

fn receiver_type_edge(result: &ResolutionResult, caller: &str, target: &str, why: &str) {
    let rows = calls_from(result, caller);
    assert!(
        rows.iter()
            .any(|(t, rung)| t == target && rung == "ReceiverType"),
        "{why}: {rows:?}"
    );
}

#[test]
fn a_private_member_of_a_java_superclass_is_not_inherited() {
    let result = resolve(&[
        (
            "a/Base.java",
            "package a;\npublic class Base { private int helper() { return 1; } }\n",
        ),
        (
            "a/Child.java",
            "package a;\npublic class Child extends Base { int go() { return helper(); } }\n",
        ),
    ]);
    no_receiver_type_edge(&result, "a/Child.java::Child.go", "J7");
}

#[test]
fn a_private_member_of_a_kotlin_superclass_is_not_inherited() {
    let result = resolve(&[
        (
            "a/Base.kt",
            "package a\nopen class Base { private fun helper(): Int = 1 }\n",
        ),
        (
            "a/Child.kt",
            "package a\nclass Child : Base() { fun go(): Int = helper() }\n",
        ),
    ]);
    no_receiver_type_edge(&result, "a/Child.kt::Child.go", "P1");
}

#[test]
fn a_private_superclass_member_does_not_beat_the_lexically_enclosing_class() {
    let result = resolve(&[
        (
            "a/Base.java",
            "package a;\npublic class Base { private int helper() { return 1; } }\n",
        ),
        (
            "a/Outer.java",
            "package a;\npublic class Outer {\n int helper() { return 2; }\n \
             class Child extends Base { int go() { return helper(); } }\n}\n",
        ),
    ]);
    no_receiver_type_edge(&result, "a/Outer.java::Child.go", "Q22");
}

#[test]
fn a_member_declared_on_two_levels_is_an_overload_set_the_rung_cannot_pick_from_kotlin() {
    let result = resolve(&[
        (
            "a/Base.kt",
            "package a\nopen class Base { fun helper(a: Int): Int = 1 }\n",
        ),
        (
            "a/Mid.kt",
            "package a\nopen class Mid : Base() { fun helper(a: Int, b: Int): Int = 2 }\n",
        ),
        (
            "a/Child.kt",
            "package a\nclass Child : Mid() { fun go(): Int = helper(1) }\n",
        ),
    ]);
    no_receiver_type_edge(&result, "a/Child.kt::Child.go", "Q1");
}

#[test]
fn a_member_declared_on_two_levels_is_an_overload_set_the_rung_cannot_pick_from_java() {
    let result = resolve(&[
        (
            "a/Base.java",
            "package a;\npublic class Base { int helper(int a) { return 1; } }\n",
        ),
        (
            "a/Mid.java",
            "package a;\npublic class Mid extends Base { int helper(int a, int b) { return 2; } }\n",
        ),
        (
            "a/Child.java",
            "package a;\npublic class Child extends Mid { int go() { return helper(1); } }\n",
        ),
    ]);
    no_receiver_type_edge(&result, "a/Child.java::Child.go", "Q2");
}

#[test]
fn a_member_found_only_through_an_interface_does_not_outrank_an_external_class_base() {
    for (name, files, caller) in [
        (
            "Q23 Java",
            vec![
                (
                    "a/Strings.java",
                    "package a;\npublic interface Strings { default String getString(int id) { return \"\"; } }\n",
                ),
                (
                    "a/C.java",
                    "package a;\nimport android.app.Service;\npublic class C extends Service implements Strings { String go() { return getString(1); } }\n",
                ),
            ],
            "a/C.java::C.go",
        ),
        (
            "Q23k Kotlin",
            vec![
                (
                    "a/Strings.kt",
                    "package a\ninterface Strings { fun getString(id: Int): String = \"\" }\n",
                ),
                (
                    "a/C.kt",
                    "package a\nimport android.app.Service\nclass C : Service(), Strings { fun go(): String = getString(1) }\n",
                ),
            ],
            "a/C.kt::C.go",
        ),
        (
            "Q24 Java through an in-repo base",
            vec![
                (
                    "a/Strings.java",
                    "package a;\npublic interface Strings { default String getString(int id) { return \"\"; } }\n",
                ),
                (
                    "a/Base.java",
                    "package a;\nimport android.app.Service;\npublic class Base extends Service { }\n",
                ),
                (
                    "a/C.java",
                    "package a;\npublic class C extends Base implements Strings { String go() { return getString(1); } }\n",
                ),
            ],
            "a/C.java::C.go",
        ),
        (
            "Q25 Kotlin through an in-repo base",
            vec![
                (
                    "a/Strings.kt",
                    "package a\ninterface Strings { fun getString(id: Int): String = \"\" }\n",
                ),
                (
                    "a/Base.kt",
                    "package a\nimport android.app.Service\nopen class Base : Service()\n",
                ),
                (
                    "a/C.kt",
                    "package a\nclass C : Base(), Strings { fun go(): String = getString(1) }\n",
                ),
            ],
            "a/C.kt::C.go",
        ),
    ] {
        let result = resolve(&files);
        no_receiver_type_edge(&result, caller, name);
    }
}

#[test]
fn what_stays_bound_in_java() {
    // J6: an interface default.
    let result = resolve(&[
        (
            "a/I.java",
            "package a;\npublic interface I { default int helper() { return 1; } }\n",
        ),
        (
            "a/C.java",
            "package a;\npublic class C implements I { int go() { return helper(); } }\n",
        ),
    ]);
    receiver_type_edge(&result, "a/C.java::C.go", "a/I.java::I.helper", "J6");

    // Q16: a same-package static namesake in an unrelated class.
    let result = resolve(&[
        (
            "a/Base.java",
            "package a;\npublic class Base { int helper() { return 1; } }\n",
        ),
        (
            "a/Other.java",
            "package a;\npublic class Other { static int helper() { return 2; } }\n",
        ),
        (
            "a/Child.java",
            "package a;\npublic class Child extends Base { int go() { return helper(); } }\n",
        ),
    ]);
    receiver_type_edge(
        &result,
        "a/Child.java::Child.go",
        "a/Base.java::Base.helper",
        "Q16",
    );

    // J1: a static import of the same name does not beat the inherited member.
    let result = resolve(&[
        (
            "a/Base.java",
            "package a;\npublic class Base { int helper() { return 1; } }\n",
        ),
        (
            "a/Child.java",
            "package a;\nimport static z.Decoy.helper;\npublic class Child extends Base { int go() { return helper(); } }\n",
        ),
        (
            "z/Decoy.java",
            "package z;\npublic class Decoy { static int helper() { return 9; } }\n",
        ),
    ]);
    receiver_type_edge(
        &result,
        "a/Child.java::Child.go",
        "a/Base.java::Base.helper",
        "J1",
    );

    // J2a: an inner class's own superclass member beats the outer class's.
    let result = resolve(&[
        (
            "a/Sup.java",
            "package a;\npublic class Sup { int helper() { return 1; } }\n",
        ),
        (
            "a/Outer.java",
            "package a;\npublic class Outer {\n int helper() { return 2; }\n \
             class In extends Sup { int go() { return helper(); } }\n}\n",
        ),
    ]);
    receiver_type_edge(
        &result,
        "a/Outer.java::In.go",
        "a/Sup.java::Sup.helper",
        "J2a",
    );
}

#[test]
fn the_java_rung_leaves_lexical_scope_to_the_rungs_after_it() {
    // J2b: an inner class with no own member calls the outer class's.
    let result = resolve(&[(
        "a/Outer.java",
        "package a;\npublic class Outer {\n int helper() { return 2; }\n \
         class In { int go() { return helper(); } }\n}\n",
    )]);
    no_receiver_type_edge(&result, "a/Outer.java::In.go", "J2b");

    // J3b: an anonymous subclass of an in-repo class, inside a class with its
    // own helper.
    let result = resolve(&[
        (
            "a/Sup.java",
            "package a;\npublic class Sup { int helper() { return 1; } }\n",
        ),
        (
            "a/Outer.java",
            "package a;\npublic class Outer {\n int helper() { return 2; }\n \
             void m() { Object o = new Sup() { int go() { return helper(); } }; }\n}\n",
        ),
    ]);
    no_receiver_type_edge(&result, "a/Outer.java::Outer.m", "J3b");

    // J3c: an anonymous Runnable calls the outer class's helper.
    let result = resolve(&[(
        "a/Outer.java",
        "package a;\npublic class Outer {\n int helper() { return 2; }\n \
         void m() { Runnable r = new Runnable() { public void run() { helper(); } }; }\n}\n",
    )]);
    no_receiver_type_edge(&result, "a/Outer.java::run", "J3c");

    // J4: a static nested class has no outer instance to call.
    let result = resolve(&[
        (
            "a/Outer.java",
            "package a;\npublic class Outer {\n int helper() { return 2; }\n \
             static class N { int go() { return helper(); } }\n}\n",
        ),
        (
            "z/Decoy.java",
            "package z;\npublic class Decoy { static int helper() { return 9; } }\n",
        ),
    ]);
    no_receiver_type_edge(&result, "a/Outer.java::N.go", "J4");
}

/// Q8, the reason Kotlin is parked (ft-87b28bb6): `helper()` inside a lambda
/// whose receiver is `Other` names `Other.helper`, not the enclosing class's
/// inherited one, and the extraction records no lambda-literal span to tell.
#[test]
fn a_kotlin_lambda_with_receiver_binds_no_member_of_the_enclosing_class() {
    let result = resolve(&[
        (
            "a/Base.kt",
            "package a\nopen class Base { fun helper(): Int = 1 }\n",
        ),
        (
            "a/Other.kt",
            "package a\nclass Other { fun helper(): Int = 3 }\n",
        ),
        (
            "a/Child.kt",
            "package a\nclass Child : Base() {\n fun build(block: Other.() -> Unit) {}\n \
             fun go() { build { helper() } }\n}\n",
        ),
    ]);
    let rows = calls_from(&result, "a/Child.kt::Child.go");
    assert!(
        rows.iter().all(|(target, _)| !target.ends_with("::helper")),
        "Q8: {rows:?}"
    );
}

#[test]
#[ignore = "ft-87b28bb6: needs Kotlin lambda-literal spans (extractor fact)"]
fn what_stays_bound_in_kotlin() {
    // K8: an interface default with no external base.
    let result = resolve(&[
        (
            "a/I.kt",
            "package a\ninterface I { fun helper(): Int = 1 }\n",
        ),
        (
            "a/C.kt",
            "package a\nclass C : I { fun go(): Int = helper() }\n",
        ),
        ("z/Decoy.kt", "package z\n\nfun helper(): Int = 99\n"),
    ]);
    receiver_type_edge(&result, "a/C.kt::C.go", "a/I.kt::I.helper", "K8");

    // K10: a generic base.
    let result = resolve(&[
        (
            "a/Base.kt",
            "package a\nopen class Base<T> { fun helper(): Int = 1 }\n",
        ),
        (
            "a/Child.kt",
            "package a\nclass Child : Base<String>() { fun go(): Int = helper() }\n",
        ),
        ("z/Decoy.kt", "package z\n\nfun helper(): Int = 99\n"),
    ]);
    receiver_type_edge(
        &result,
        "a/Child.kt::Child.go",
        "a/Base.kt::Base.helper",
        "K10",
    );

    // Q14: the inherited member beats a same-package top-level function.
    let result = resolve(&[
        (
            "a/Base.kt",
            "package a\nopen class Base { fun helper(): Int = 1 }\n",
        ),
        ("a/Top.kt", "package a\nfun helper(): Int = 2\n"),
        (
            "a/Child.kt",
            "package a\nclass Child : Base() { fun go(): Int = helper() }\n",
        ),
    ]);
    receiver_type_edge(
        &result,
        "a/Child.kt::Child.go",
        "a/Base.kt::Base.helper",
        "Q14",
    );

    // A Kotlin protected member is inherited.
    let result = resolve(&[
        (
            "a/Base.kt",
            "package a\nopen class Base { protected fun helper(): Int = 1 }\n",
        ),
        (
            "a/Child.kt",
            "package a\nclass Child : Base() { fun go(): Int = helper() }\n",
        ),
    ]);
    receiver_type_edge(
        &result,
        "a/Child.kt::Child.go",
        "a/Base.kt::Base.helper",
        "Q20",
    );
}
