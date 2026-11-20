#![cfg(feature = "parse")]
//! ft-0e6cc90f criterion 9. `has_external_supertype` keeps a Kotlin class that
//! extends `android.app.Service` from binding a bare `getString(..)` to a repo
//! top-level `getString`. It must not also cost recall:
//!
//! * a Kotlin or Java class that inherits an in-repo member through an in-repo
//!   base that itself extends an external class calls that member bare, and the
//!   member is the receiver's own, found before the global rung is asked.
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
