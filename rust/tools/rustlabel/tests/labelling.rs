//! The labeller against a scripted server: what becomes a label, what is
//! recorded as unknown, and what is skipped, for answered, unanswered,
//! external, failing and timed-out sites. A check that could not run must
//! never read as one that passed, so every non-label outcome is counted.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use devmap_resolve::lsp::{LspAnswer, LspLocation, LspQuery};
use rustlabel::labels::Labels;
use rustlabel::run::Limits;
use rustlabel::run::{label, LocateFailure, Locator, Outcome};

/// Answers keyed by (query, file, line, character); anything not scripted is
/// an empty answer, which is what a server with nothing to say returns.
#[derive(Default)]
struct Stub {
    answers: BTreeMap<(u8, String, u32, u32), Result<LspAnswer, LocateFailure>>,
    asked: Vec<(LspQuery, String, u32, u32)>,
    /// Answers handed out in order instead, for a script that fails partway.
    sequence: Vec<Result<LspAnswer, LocateFailure>>,
}

fn key(query: LspQuery) -> u8 {
    match query {
        LspQuery::Definition => 0,
        LspQuery::Implementation => 1,
        LspQuery::Declaration => 2,
    }
}

impl Locator for Stub {
    fn locate(
        &mut self,
        query: LspQuery,
        file: &str,
        _source: &str,
        line: u32,
        character: u32,
    ) -> Result<LspAnswer, LocateFailure> {
        self.asked.push((query, file.to_string(), line, character));
        if !self.sequence.is_empty() {
            return self.sequence.remove(0);
        }
        self.answers
            .get(&(key(query), file.to_string(), line, character))
            .cloned()
            .unwrap_or_else(|| Ok(LspAnswer::default()))
    }
}

/// Zero-based line and UTF-16 column of `byte`.
fn pos(source: &str, byte: usize) -> (u32, u32) {
    let before = &source[..byte];
    let line = before.matches('\n').count() as u32;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, source[start..byte].encode_utf16().count() as u32)
}

fn at(source: &str, needle: &str, nth: usize) -> usize {
    source.match_indices(needle).nth(nth).expect(needle).0
}

impl Stub {
    fn answer(
        &mut self,
        query: LspQuery,
        file: &str,
        source: &str,
        byte: usize,
        answer: Result<LspAnswer, LocateFailure>,
    ) {
        let (line, character) = pos(source, byte);
        self.answers
            .insert((key(query), file.to_string(), line, character), answer);
    }
}

fn loc(file: &str, source: &str, byte: usize) -> LspLocation {
    let (line, character) = pos(source, byte);
    LspLocation {
        file: file.to_string(),
        line,
        character,
    }
}

fn found(locations: Vec<LspLocation>) -> Result<LspAnswer, LocateFailure> {
    Ok(LspAnswer {
        locations,
        ..LspAnswer::default()
    })
}

fn external() -> Result<LspAnswer, LocateFailure> {
    Ok(LspAnswer {
        outside_repo: 1,
        ..LspAnswer::default()
    })
}

fn limits() -> Limits {
    Limits {
        max_sites: 10_000,
        deadline: Instant::now() + Duration::from_secs(60),
        references: true,
        trace: true,
    }
}

fn run(files: &[(&str, &str)], stub: &mut Stub, limits: &Limits) -> Outcome {
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(p, s)| (p.to_string(), s.to_string()))
        .collect();
    label(&files, stub, limits)
}

const A: &str = "trait Speak { fn speak(&self); }\nstruct Dog;\nimpl Speak for Dog { fn speak(&self) {} }\nfn helper() {}\nfn caller(d: &dyn Speak) {\n    d.speak();\n    helper();\n    d.len();\n}\n";

fn rows(labels: &Labels) -> Vec<(String, String, String, String)> {
    labels
        .recall
        .iter()
        .map(|r| {
            (
                r.construct.clone(),
                r.caller.clone(),
                r.name.clone(),
                r.truth.clone(),
            )
        })
        .collect()
}

fn row(construct: &str, caller: &str, name: &str, truth: &str) -> (String, String, String, String) {
    (construct.into(), caller.into(), name.into(), truth.into())
}

/// Every enumerated site ends in exactly one bucket.
fn assert_accounted(o: &Outcome) {
    let r = &o.report;
    let unknown: usize = r.unknown.values().sum();
    let skipped: usize = r.skipped.values().sum();
    assert_eq!(
        r.sites_enumerated,
        r.labelled + r.external + unknown + skipped,
        "{r:?}"
    );
}

fn scripted_a() -> Stub {
    let mut stub = Stub::default();
    let trait_decl = at(A, "speak(&self);", 0);
    stub.answer(
        LspQuery::Definition,
        "a.rs",
        A,
        at(A, "speak();", 0),
        found(vec![loc("a.rs", A, trait_decl)]),
    );
    stub.answer(
        LspQuery::Definition,
        "a.rs",
        A,
        at(A, "helper();", 0),
        found(vec![loc("a.rs", A, at(A, "helper()", 0))]),
    );
    stub.answer(
        LspQuery::Implementation,
        "a.rs",
        A,
        trait_decl,
        found(vec![loc("a.rs", A, at(A, "speak(&self) {}", 0))]),
    );
    stub
}

#[test]
fn a_dispatch_through_a_trait_names_the_declaration_and_every_implementation() {
    let mut stub = scripted_a();
    let o = run(&[("a.rs", A)], &mut stub, &limits());
    assert_eq!(
        rows(&o.labels),
        vec![
            row(
                "non_self",
                "a.rs::caller",
                "speak",
                "a.rs::Speak.speak|a.rs::Dog.speak"
            ),
            row("free_self", "a.rs::caller", "helper", "a.rs::helper"),
        ]
    );
    assert_eq!(o.report.labelled, 2);
    assert_eq!(o.report.implementation_queries, 1);
    // `d.len()` names nothing in the repository, so it was never asked.
    assert_eq!(o.report.skipped["not_in_repo_name"], 1);
    assert_eq!(stub.asked.len(), 3, "{:?}", stub.asked);
    assert_accounted(&o);

    let oracle = &o.labels.oracle[0];
    assert_eq!(oracle.language, "rust");
    assert!(oracle.covered_callers.contains(&"a.rs::caller".to_string()));
    assert!(oracle.covered_callers.contains(&"a.rs".to_string()));
    let uses: Vec<(&str, &str, &[String])> = oracle
        .uses
        .iter()
        .map(|u| (u.caller.as_str(), u.callee.as_str(), u.impls.as_slice()))
        .collect();
    assert_eq!(
        uses,
        vec![
            (
                "a.rs::caller",
                "a.rs::Speak.speak",
                &["a.rs::Dog.speak".to_string()][..]
            ),
            ("a.rs::caller", "a.rs::helper", &[][..]),
        ]
    );
}

#[test]
fn an_unanswered_site_is_unknown_never_a_label_and_uncovers_its_caller() {
    let mut stub = scripted_a();
    // The server says nothing about `helper()`.
    stub.answers.remove(&(
        key(LspQuery::Definition),
        "a.rs".to_string(),
        pos(A, at(A, "helper();", 0)).0,
        pos(A, at(A, "helper();", 0)).1,
    ));
    let o = run(&[("a.rs", A)], &mut stub, &limits());
    assert_eq!(o.report.unknown["no_answer"], 1, "{:?}", o.report);
    assert!(!rows(&o.labels).iter().any(|r| r.2 == "helper"));
    // One site of `caller` is unread, so none of `caller`'s edges is judged.
    assert!(!o.labels.oracle[0]
        .covered_callers
        .contains(&"a.rs::caller".to_string()));
    assert!(o.report.uncovered_callers >= 1);
    assert!(o.labels.oracle[0]
        .uses
        .iter()
        .all(|u| u.caller != "a.rs::caller"));
    assert_accounted(&o);
}

#[test]
fn a_definition_outside_the_repository_is_answered_but_labels_nothing() {
    let mut stub = scripted_a();
    stub.answer(
        LspQuery::Definition,
        "a.rs",
        A,
        at(A, "helper();", 0),
        external(),
    );
    let o = run(&[("a.rs", A)], &mut stub, &limits());
    assert_eq!(o.report.external, 1);
    assert!(!rows(&o.labels).iter().any(|r| r.2 == "helper"));
    assert!(o.labels.oracle[0]
        .covered_callers
        .contains(&"a.rs::caller".to_string()));
    assert_accounted(&o);
}

const B: &str = "fn helper() {}\nfn caller() {\n    helper();\n    helper();\n    helper();\n}\n";

fn all_helper_decl(stub: &mut Stub) {
    for nth in 0..3 {
        stub.answer(
            LspQuery::Definition,
            "b.rs",
            B,
            at(B, "helper();", nth),
            found(vec![loc("b.rs", B, at(B, "helper()", 0))]),
        );
    }
}

#[test]
fn a_failed_request_is_one_unknown_and_the_run_goes_on() {
    let mut stub = Stub::default();
    let decl = found(vec![loc("b.rs", B, at(B, "helper()", 0))]);
    stub.sequence = vec![
        Err(LocateFailure::Request("-32801".into())),
        decl.clone(),
        decl,
    ];
    let o = run(&[("b.rs", B)], &mut stub, &limits());
    assert_eq!(o.report.unknown["request_error"], 1, "{:?}", o.report);
    assert_eq!(o.report.labelled, 2);
    assert_accounted(&o);
}

#[test]
fn a_dead_session_makes_every_remaining_site_unknown_without_asking_again() {
    let mut stub = Stub::default();
    let decl = found(vec![loc("b.rs", B, at(B, "helper()", 0))]);
    stub.sequence = vec![decl, Err(LocateFailure::Fatal("timed_out".into()))];
    let o = run(&[("b.rs", B)], &mut stub, &limits());
    assert_eq!(o.report.labelled, 1);
    assert_eq!(o.report.unknown["session_lost"], 2, "{:?}", o.report);
    assert_eq!(stub.asked.len(), 2, "no request after the session died");
    assert!(o.report.session_failure.as_deref() == Some("timed_out"));
    assert_accounted(&o);
}

#[test]
fn a_run_past_its_deadline_asks_nothing_and_says_so() {
    let mut stub = Stub::default();
    all_helper_decl(&mut stub);
    let expired = Limits {
        deadline: Instant::now() - Duration::from_secs(1),
        ..limits()
    };
    let o = run(&[("b.rs", B)], &mut stub, &expired);
    assert!(stub.asked.is_empty());
    assert_eq!(o.report.unknown["deadline"], 3, "{:?}", o.report);
    assert!(o.labels.recall.is_empty());
    assert_accounted(&o);
}

#[test]
fn a_site_cap_reports_both_numbers_and_never_reads_as_full_coverage() {
    let mut stub = Stub::default();
    all_helper_decl(&mut stub);
    let capped = Limits {
        max_sites: 1,
        ..limits()
    };
    let o = run(&[("b.rs", B)], &mut stub, &capped);
    assert_eq!(o.report.sites_enumerated, 3);
    assert_eq!(o.report.asked, 1);
    assert_eq!(o.report.skipped["over_cap"], 2, "{:?}", o.report);
    assert!(!o.labels.oracle[0]
        .covered_callers
        .contains(&"b.rs::caller".to_string()));
    assert_accounted(&o);
}

/// `cb()` calls a parameter, and the server's answer is the parameter's
/// declaration, inside `caller`. The enclosing function contains that position,
/// so a labeller that named "the symbol around the answer" would record
/// `caller` calling itself.
#[test]
fn an_answer_inside_a_function_that_is_not_its_name_is_not_a_function_target() {
    const C: &str = "fn cb() {}\nfn caller(cb: fn()) {\n    cb();\n}\n";
    let mut stub = Stub::default();
    stub.answer(
        LspQuery::Definition,
        "c.rs",
        C,
        at(C, "cb();", 0),
        found(vec![loc("c.rs", C, at(C, "cb: fn", 0))]),
    );
    let o = run(&[("c.rs", C)], &mut stub, &limits());
    assert!(o.labels.recall.is_empty(), "{:?}", o.labels.recall);
    assert_eq!(o.report.skipped["non_function_target"], 1, "{:?}", o.report);
    assert_accounted(&o);
}

#[test]
fn an_answer_that_mixes_a_function_and_something_else_is_unknown() {
    const C: &str = "fn cb() {}\nfn caller(cb: fn()) {\n    cb();\n}\n";
    let mut stub = Stub::default();
    stub.answer(
        LspQuery::Definition,
        "c.rs",
        C,
        at(C, "cb();", 0),
        found(vec![
            loc("c.rs", C, at(C, "cb()", 0)),
            loc("c.rs", C, at(C, "cb: fn", 0)),
        ]),
    );
    let o = run(&[("c.rs", C)], &mut stub, &limits());
    assert!(o.labels.recall.is_empty());
    assert_eq!(o.report.unknown["mixed_targets"], 1, "{:?}", o.report);
    assert_accounted(&o);
}

#[test]
fn a_target_in_a_file_the_run_never_read_is_unknown_not_dropped() {
    let mut stub = Stub::default();
    stub.answer(
        LspQuery::Definition,
        "b.rs",
        B,
        at(B, "helper();", 0),
        found(vec![LspLocation {
            file: "generated.rs".into(),
            line: 0,
            character: 3,
        }]),
    );
    let o = run(&[("b.rs", B)], &mut stub, &limits());
    assert_eq!(o.report.unknown["target_unnamed"], 1, "{:?}", o.report);
    assert!(o.labels.recall.is_empty());
}

#[test]
fn a_trait_whose_implementations_were_not_answered_is_unknown() {
    let mut stub = scripted_a();
    stub.answer(
        LspQuery::Implementation,
        "a.rs",
        A,
        at(A, "speak(&self);", 0),
        Err(LocateFailure::Request("internal error".into())),
    );
    let o = run(&[("a.rs", A)], &mut stub, &limits());
    assert_eq!(o.report.unknown["impls_unanswered"], 1, "{:?}", o.report);
    assert!(!rows(&o.labels).iter().any(|r| r.2 == "speak"));
    assert_accounted(&o);
}

#[test]
fn a_call_inside_a_macro_uncovers_its_caller() {
    const M: &str = "fn helper() {}\nfn c() {\n    println!(\"{}\", helper());\n}\nfn d() {\n    println!(\"hi\");\n}\n";
    let mut stub = Stub::default();
    let o = run(&[("m.rs", M)], &mut stub, &limits());
    let covered = &o.labels.oracle[0].covered_callers;
    assert!(!covered.contains(&"m.rs::c".to_string()), "{covered:?}");
    assert!(covered.contains(&"m.rs::d".to_string()), "{covered:?}");
    assert_eq!(o.report.macro_invocations, 2);
}

/// A macro the corpus defines expands to calls nobody wrote at the invocation,
/// and its body holds calls the grammar never parsed. DevMap resolves both, so
/// an oracle that called either caller fully read would judge those edges
/// wrong. (Found by scoring the Rust fixture: `emit_c12`, `via_macro_body_c41`.)
#[test]
fn a_macro_the_corpus_defines_uncovers_its_definition_and_every_invocation() {
    const D: &str = "fn emit() {}\nmacro_rules! log {\n    ($m:expr) => { $crate::emit() };\n}\nfn user() {\n    log!(1);\n}\nfn bystander() {\n    other!(1);\n}\n";
    let mut stub = Stub::default();
    let o = run(&[("d.rs", D)], &mut stub, &limits());
    let covered = &o.labels.oracle[0].covered_callers;
    assert!(
        !covered.contains(&"d.rs".to_string()),
        "the definition's body is the file's: {covered:?}"
    );
    assert!(!covered.contains(&"d.rs::user".to_string()), "{covered:?}");
    assert!(
        covered.contains(&"d.rs::bystander".to_string()),
        "{covered:?}"
    );
    assert_eq!(o.report.user_macro_invocations, 1);
}

const TRAIT_SRC: &str = "trait Render { fn render(&self); }\nstruct Sq;\nimpl Render for Sq { fn render(&self) {} }\nfn make() -> impl Render { Sq }\nfn c() {\n    make().render();\n}\n";

fn script_sq(stub: &mut Stub, declaration: Option<Result<LspAnswer, LocateFailure>>) {
    let impl_method = at(TRAIT_SRC, "render(&self) {}", 0);
    stub.answer(
        LspQuery::Definition,
        "t.rs",
        TRAIT_SRC,
        at(TRAIT_SRC, "make().render", 0),
        found(vec![loc("t.rs", TRAIT_SRC, at(TRAIT_SRC, "make()", 0))]),
    );
    stub.answer(
        LspQuery::Definition,
        "t.rs",
        TRAIT_SRC,
        at(TRAIT_SRC, "render();", 0),
        found(vec![loc("t.rs", TRAIT_SRC, impl_method)]),
    );
    if let Some(declaration) = declaration {
        stub.answer(
            LspQuery::Declaration,
            "t.rs",
            TRAIT_SRC,
            impl_method,
            declaration,
        );
    }
}

/// rust-analyzer sees through `impl Render` to `Sq.render`; DevMap, which keeps
/// the written type, edges to `Render.render`. Both describe the call, so a
/// call bound to a trait implementation also accepts the trait's declaration.
/// (Fixture C07.)
#[test]
fn a_call_bound_to_a_trait_implementation_also_accepts_the_trait_declaration() {
    let mut stub = Stub::default();
    script_sq(
        &mut stub,
        Some(found(vec![loc(
            "t.rs",
            TRAIT_SRC,
            at(TRAIT_SRC, "render(&self);", 0),
        )])),
    );
    let o = run(&[("t.rs", TRAIT_SRC)], &mut stub, &limits());
    assert_eq!(
        rows(&o.labels),
        vec![
            row(
                "non_self",
                "t.rs::c",
                "render",
                "t.rs::Sq.render|t.rs::Render.render"
            ),
            row("free_self", "t.rs::c", "make", "t.rs::make"),
        ]
    );
    let uses: Vec<(&str, &[String])> = o.labels.oracle[0]
        .uses
        .iter()
        .map(|u| (u.callee.as_str(), u.impls.as_slice()))
        .collect();
    assert_eq!(
        uses,
        vec![
            ("t.rs::Sq.render", &["t.rs::Render.render".to_string()][..]),
            ("t.rs::make", &[][..]),
        ]
    );
    assert_eq!(o.report.declaration_queries, 1);
    assert_accounted(&o);
}

/// The declaration is an addition to a label that is already true. Asking
/// failed, or it named nothing, or it named the function itself: the label
/// stands as the strict one.
#[test]
fn a_declaration_that_is_missing_or_unanswered_leaves_the_strict_label() {
    for declaration in [
        None,
        Some(Err(LocateFailure::Request("internal error".into()))),
        Some(found(vec![loc(
            "t.rs",
            TRAIT_SRC,
            at(TRAIT_SRC, "render(&self) {}", 0),
        )])),
    ] {
        let mut stub = Stub::default();
        script_sq(&mut stub, declaration);
        let o = run(&[("t.rs", TRAIT_SRC)], &mut stub, &limits());
        assert_eq!(
            rows(&o.labels),
            vec![
                row("non_self", "t.rs::c", "render", "t.rs::Sq.render"),
                row("free_self", "t.rs::c", "make", "t.rs::make"),
            ]
        );
        assert_accounted(&o);
    }
}

/// `generate_handler![cmd]` names `cmd` without calling it; DevMap reads the
/// token as a reference. Only a macro that spells a corpus function's name is a
/// reason to distrust its caller. (Fixture C39.)
#[test]
fn a_function_named_inside_any_macro_uncovers_the_caller() {
    const H: &str = "fn cmd() {}\nfn main() {\n    register![cmd];\n}\nfn quiet() {\n    register![other];\n}\n";
    let mut stub = Stub::default();
    let o = run(&[("h.rs", H)], &mut stub, &limits());
    let covered = &o.labels.oracle[0].covered_callers;
    assert!(!covered.contains(&"h.rs::main".to_string()), "{covered:?}");
    assert!(covered.contains(&"h.rs::quiet".to_string()), "{covered:?}");
    assert_eq!(o.report.opaque_macro_sites, 1);
}

/// `const FPTR: fn() = helper;` names `helper` where a value is expected, at
/// file scope.
#[test]
fn a_function_named_in_a_const_or_static_initializer_is_a_value_site() {
    const K: &str = "fn helper() {}\nconst F: fn() = helper;\nstatic G: fn() = helper;\n";
    let mut stub = Stub::default();
    for nth in 0..2 {
        stub.answer(
            LspQuery::Definition,
            "k.rs",
            K,
            at(K, "= helper;", nth) + 2,
            found(vec![loc("k.rs", K, at(K, "helper()", 0))]),
        );
    }
    let o = run(&[("k.rs", K)], &mut stub, &limits());
    assert_eq!(
        rows(&o.labels),
        vec![
            row("free_self", "k.rs", "helper", "k.rs::helper"),
            row("free_self", "k.rs", "helper", "k.rs::helper"),
        ]
    );
}

/// DevMap draws edges for scope-end `Drop`, `.into()` and `for`; an oracle that
/// only reads written sites judges them wrong. Say so in the report, by name,
/// so a precision number over such a corpus is read as a lower bound.
#[test]
fn the_report_names_the_implicit_callees_the_corpus_declares() {
    const T: &str = "struct R;\nimpl Drop for R {\n    fn drop(&mut self) {}\n}\nimpl From<u8> for R {\n    fn from(_: u8) -> R { R }\n}\nfn plain() {}\n";
    let mut stub = Stub::default();
    let o = run(&[("t.rs", T)], &mut stub, &limits());
    assert_eq!(
        o.report.implicit_callee_names,
        BTreeMap::from([("drop".to_string(), 1), ("from".to_string(), 1)])
    );
}

#[test]
fn the_trace_says_what_became_of_every_site() {
    let mut stub = scripted_a();
    let o = run(&[("a.rs", A)], &mut stub, &limits());
    assert_eq!(
        o.trace,
        vec![
            "a.rs:6 speak non_self a.rs::Speak.speak|a.rs::Dog.speak".to_string(),
            "a.rs:7 helper free_self a.rs::helper".to_string(),
            "a.rs:8 len skipped:not_in_repo_name".to_string(),
        ]
    );
    let silent = Limits {
        trace: false,
        ..limits()
    };
    let mut stub = scripted_a();
    assert!(run(&[("a.rs", A)], &mut stub, &silent).trace.is_empty());
}

#[test]
fn a_function_in_an_inline_module_is_named_without_the_module() {
    const I: &str = "mod m {\n    pub fn f() {}\n}\nfn c() {\n    m::f();\n}\n";
    let mut stub = Stub::default();
    stub.answer(
        LspQuery::Definition,
        "i.rs",
        I,
        at(I, "f();", 0),
        found(vec![loc("i.rs", I, at(I, "f()", 0))]),
    );
    let o = run(&[("i.rs", I)], &mut stub, &limits());
    assert_eq!(
        rows(&o.labels),
        vec![row("free_self", "i.rs::c", "f", "i.rs::f")]
    );
}

#[test]
fn an_overloaded_operator_is_a_site_and_a_primitive_one_is_not_asked() {
    const O: &str = "struct V;\nimpl Add for V {\n    fn add(self, o: V) -> V { V }\n}\nfn c(a: V, b: V) {\n    a + b;\n    a - b;\n}\n";
    let mut stub = Stub::default();
    stub.answer(
        LspQuery::Definition,
        "o.rs",
        O,
        at(O, "+ b", 0),
        found(vec![loc("o.rs", O, at(O, "add(self", 0))]),
    );
    let o = run(&[("o.rs", O)], &mut stub, &limits());
    assert_eq!(
        rows(&o.labels),
        vec![row("non_self", "o.rs::c", "+", "o.rs::V.add")]
    );
    // No function called `sub` exists, so `-` cannot reach one.
    assert_eq!(o.report.skipped["not_in_repo_name"], 1, "{:?}", o.report);
    let definitions = stub
        .asked
        .iter()
        .filter(|a| a.0 == LspQuery::Definition)
        .count();
    assert_eq!(definitions, 1);
    assert_accounted(&o);
}

/// rust-analyzer answers for the token at an offset, and `[` sits between `t`
/// and `0`: asked there, it answers `t`'s declaration. Fixture C26 showed it
/// (`Table.index`). An operator answer that is not a function the operator
/// names says nothing about the operator, so it is unknown and uncovers the
/// caller, where the same answer to a plain call is "not a function".
#[test]
fn an_operator_answer_that_is_not_an_operator_method_is_unknown_not_skipped() {
    const X: &str = "struct T;\nimpl Index<usize> for T {\n    fn index(&self, i: usize) -> &u8 { &0 }\n}\nfn add() {}\nfn c(t: &T) {\n    t[0];\n    add();\n}\n";
    let mut stub = Stub::default();
    // The index operator answered with the parameter `t`.
    stub.answer(
        LspQuery::Definition,
        "x.rs",
        X,
        at(X, "[0]", 0),
        found(vec![loc("x.rs", X, at(X, "t: &T", 0))]),
    );
    // A plain call answered with the same kind of non-function.
    stub.answer(
        LspQuery::Definition,
        "x.rs",
        X,
        at(X, "add();", 0),
        found(vec![loc("x.rs", X, at(X, "t: &T", 0))]),
    );
    let o = run(&[("x.rs", X)], &mut stub, &limits());
    assert!(o.labels.recall.is_empty(), "{:?}", o.labels.recall);
    assert_eq!(o.report.unknown["operator_unresolved"], 1, "{:?}", o.report);
    assert_eq!(o.report.skipped["non_function_target"], 1, "{:?}", o.report);
    assert!(!o.labels.oracle[0]
        .covered_callers
        .contains(&"x.rs::c".to_string()));
    assert_accounted(&o);
}

/// `foo()?` asked at the `?` can come back as `foo` (the `)` beside it). A
/// real function that is not one the operator's trait names is not the
/// operator's callee.
#[test]
fn an_operator_answered_with_an_unrelated_function_is_not_labelled() {
    const X: &str = "fn from_cache() {}\nfn from() {}\nfn c() {\n    foo()?;\n}\n";
    let mut stub = Stub::default();
    stub.answer(
        LspQuery::Definition,
        "x.rs",
        X,
        at(X, "?;", 0),
        found(vec![loc("x.rs", X, at(X, "from_cache", 0))]),
    );
    let o = run(&[("x.rs", X)], &mut stub, &limits());
    assert!(o.labels.recall.is_empty(), "{:?}", o.labels.recall);
    assert_eq!(o.report.unknown["operator_unresolved"], 1, "{:?}", o.report);
}

#[test]
fn an_alias_keeps_a_renamed_callee_in_play() {
    const R: &str = "use other::real as renamed;\nfn real() {}\nfn c() {\n    renamed();\n}\n";
    let mut stub = Stub::default();
    stub.answer(
        LspQuery::Definition,
        "r.rs",
        R,
        at(R, "renamed();", 0),
        found(vec![loc("r.rs", R, at(R, "real()", 0))]),
    );
    let o = run(&[("r.rs", R)], &mut stub, &limits());
    assert_eq!(
        rows(&o.labels),
        vec![row("free_self", "r.rs::c", "renamed", "r.rs::real")]
    );
}

#[test]
fn the_encoding_is_the_labels_file_shape_and_the_same_bytes_every_time() {
    let encode = || {
        let mut stub = scripted_a();
        run(&[("a.rs", A)], &mut stub, &limits())
            .labels
            .encode()
            .unwrap()
    };
    let first = encode();
    assert_eq!(first, encode());
    assert!(first.ends_with(b"\n"));
    let value: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(value["dead"], serde_json::json!([]));
    assert_eq!(value["recall"][0]["language"], "rust");
    assert_eq!(value["recall"][0]["construct"], "non_self");
    assert_eq!(value["oracle"][0]["language"], "rust");
    assert!(value["oracle"][0]["uses"][0]["impls"].is_array());
}
