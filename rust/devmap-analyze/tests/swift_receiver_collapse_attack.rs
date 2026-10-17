//! The `Ctor(…)` receiver collapse (f2187b0d) must touch only a bare-name,
//! parenthesised construction that is multi-line or longer than 96 bytes. Every
//! other receiver shape must not be rewritten to `Name(…)`, and no other language
//! is affected.

use devmap_extract::extract_file;

fn receivers(path: &str, source: &str, callee: &str) -> Vec<String> {
    extract_file(path, source)
        .calls
        .into_iter()
        .filter(|c| c.callee_name == callee)
        .filter_map(|c| c.receiver_expr)
        .collect()
}

#[test]
fn only_a_bare_name_construction_collapses() {
    let long = "x".repeat(120);
    // (source, callee, the one shape of receiver that would be wrong)
    let cases: Vec<(String, &str)> = vec![
        (
            "func f() {\n    \"a(\n b)\".hasPrefix(\"a\")\n}".into(),
            "hasPrefix",
        ),
        ("func f() {\n    (a,\n b).0.run()\n}".into(), "run"),
        (
            "func f() {\n    { () -> Svc in\n        Svc() }().run()\n}".into(),
            "run",
        ),
        (
            "func f() {\n    Svc<Int>(\n        a: 1\n    ).run()\n}".into(),
            "run",
        ),
        (
            "func f() {\n    Mod.Svc(\n        a: 1\n    ).run()\n}".into(),
            "run",
        ),
        (
            format!("func f() {{ (try? Svc(a: \"{long}\"))?.run() }}"),
            "run",
        ),
    ];
    for (source, callee) in &cases {
        let got = receivers("a.swift", source, callee);
        eprintln!(
            "RECEIVER {:?} => {got:?}",
            source.lines().nth(1).unwrap_or(source)
        );
        assert!(got.iter().all(|t| t != "Svc(…)"), "{source:?} -> {got:?}");
    }
}

#[test]
fn generic_construction_does_not_collapse_to_the_bare_constructor() {
    // `Svc<Int>(…)` is a construction of `Svc<Int>`; both `Svc(…)` and the old
    // clamp are readable by the resolver, but they must agree on the type.
    let got = receivers(
        "a.swift",
        "func f() {\n    Svc<Int>(\n        a: 1\n    ).run()\n}",
        "run",
    );
    eprintln!("generic => {got:?}");
    assert_eq!(got.len(), 1);
}

#[test]
fn try_and_await_prefixed_multi_line_constructions_report_what_they_report() {
    for kw in ["try", "await", "try await"] {
        let src = format!("func f() async throws {{\n    {kw} Svc(\n        a: 1\n    ).run()\n}}");
        let got = receivers("a.swift", &src, "run");
        eprintln!("{kw} => {got:?}");
        assert_eq!(got.len(), 1, "{kw}");
        // a receiver starting with the keyword can never be typed
        assert!(
            !got[0].starts_with("try") && !got[0].starts_with("await"),
            "{kw}: the keyword is part of the receiver text: {got:?}"
        );
    }
}

#[test]
fn a_single_line_short_construction_is_unchanged() {
    assert_eq!(
        receivers("a.swift", "func f() { Svc(a: 1).run() }", "run"),
        vec!["Svc(a: 1)".to_string()]
    );
    assert_eq!(
        receivers("a.swift", "func f() { Svc.shared.run() }", "run"),
        vec!["Svc.shared".to_string()]
    );
}

#[test]
fn other_languages_never_see_the_collapse() {
    for (path, src) in [
        ("a.kt", "fun f() {\n    Svc(\n        1\n    ).run()\n}\n"),
        ("a.ts", "function f() {\n  new Svc(\n    1\n  ).run();\n}\n"),
        ("a.py", "def f():\n    Svc(\n        1\n    ).run()\n"),
        (
            "a.go",
            "package a\nfunc f() {\n\tNewSvc(\n\t\t1,\n\t).Run()\n}\n",
        ),
        (
            "a.rs",
            "fn f() {\n    Svc::new(\n        1\n    ).run();\n}\n",
        ),
    ] {
        let found: Vec<String> = extract_file(path, src)
            .calls
            .into_iter()
            .filter_map(|c| c.receiver_expr)
            .collect();
        eprintln!("LANG {path}: {found:?}");
        assert!(
            found.iter().all(|t| !t.ends_with("(…)")),
            "{path}: {found:?}"
        );
    }
}
