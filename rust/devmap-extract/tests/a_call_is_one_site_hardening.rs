//! Adversarial cover for "a call is one attribution site".
//!
//! `drop_duplicate_callee_names` is a *structural* rule — same name, same end
//! byte, containing span — applied to every language at once. That is its
//! value and its risk: a rule with no grammar table cannot rot when a grammar
//! is added, and it also cannot be reviewed one grammar at a time. So it is
//! attacked here from both directions.
//!
//! **Over-dropping** is the expensive failure. Refusing a receiver's own
//! reference made every method receiver invisible to the graph once before, and
//! a module-level singleton used the way singletons are used read as dead in
//! eleven files at once. Every case below that asserts a reference *survives* is
//! guarding that.
//!
//! **Under-dropping** is the failure the rule exists to fix, and the sweep over
//! the registry is what keeps it fixed: a grammar added tomorrow with a call
//! extractor and no snippet here fails this file rather than quietly
//! reintroducing the duplicate.

use devmap_extract::extract_file;
use devmap_extract::languages::{Capability, LANGUAGE_SPECS};
use devmap_extract::model::{Extraction, ReferenceKind};

fn refs_named<'a>(extraction: &'a Extraction, name: &str) -> Vec<&'a ReferenceKind> {
    extraction
        .references
        .iter()
        .filter(|r| r.name == name)
        .map(|r| &r.kind)
        .collect()
}

fn count(extraction: &Extraction, name: &str, kind: ReferenceKind) -> usize {
    refs_named(extraction, name)
        .into_iter()
        .filter(|k| **k == kind)
        .count()
}

/// One snippet per grammar that declares a call extractor.
///
/// Each makes exactly **one** call whose callee is spelled `zzuniq`, a token
/// that appears nowhere else in the snippet. The invariant is then independent
/// of how the rule is implemented: the extraction must hold exactly one
/// reference named `zzuniq`. Zero means the callee's own reference was dropped
/// along with the duplicate; two means the duplicate survived.
const CALL_SNIPPETS: &[(&str, &str, &str)] = &[
    (
        "typescript",
        "a.ts",
        "export function g(r: H) { r.zzuniq(); }",
    ),
    ("tsx", "a.tsx", "export function g(r: H) { r.zzuniq(); }"),
    ("javascript", "a.js", "export function g(r) { r.zzuniq(); }"),
    ("python", "a.py", "def g(r):\n    r.zzuniq()\n"),
    ("go", "a.go", "package p\nfunc G(r H) { r.zzuniq() }"),
    ("rust", "a.rs", "fn g(r: H) { r.zzuniq(); }"),
    ("java", "a.java", "class D { void g(H r) { r.zzuniq(); } }"),
    ("csharp", "a.cs", "class D { void G(H r) { r.zzuniq(); } }"),
    ("php", "a.php", "<?php function g($r) { $r->zzuniq(); }"),
    ("ruby", "a.rb", "def g(r)\n  r.zzuniq\nend\n"),
    ("c", "a.c", "void g(H r) { r.zzuniq(); }"),
    ("cpp", "a.cpp", "void g(H r) { r.zzuniq(); }"),
    ("objc", "a.m", "void g(H *r) { [r zzuniq]; }"),
    ("cuda", "a.cu", "void g(H r) { r.zzuniq(); }"),
    ("swift", "a.swift", "func g(r: H) { r.zzuniq() }"),
    ("kotlin", "a.kt", "fun g(r: H) { r.zzuniq() }"),
    (
        "scala",
        "a.scala",
        "object D { def g(r: H): Unit = { r.zzuniq() } }",
    ),
    ("dart", "a.dart", "void g(H r) { r.zzuniq(); }"),
    (
        "svelte",
        "a.svelte",
        "<script>export function g(r) { r.zzuniq(); }</script>\n<p>x</p>\n",
    ),
    (
        "vue",
        "a.vue",
        "<script>export function g(r) { r.zzuniq(); }</script>\n<template><p>x</p></template>\n",
    ),
    (
        "astro",
        "a.astro",
        "---\nexport function g(r) { r.zzuniq(); }\n---\n<p>x</p>\n",
    ),
    (
        "liquid",
        "a.liquid",
        "<script>function g(r) { r.zzuniq(); }</script>\n",
    ),
    (
        "pascal",
        "a.pas",
        "program D;\nbegin\n  r.zzuniq();\nend.\n",
    ),
    ("lua", "a.lua", "local function g(r)\n  r:zzuniq()\nend\n"),
    ("luau", "a.luau", "local function g(r)\n  r:zzuniq()\nend\n"),
    ("r", "a.r", "g <- function(r) {\n  r$zzuniq()\n}\n"),
    ("erlang", "a.erl", "-module(a).\ng(R) -> zzmod:zzuniq(R).\n"),
    (
        "solidity",
        "a.sol",
        "contract D { function g(H r) public { r.zzuniq(); } }",
    ),
    ("nix", "a.nix", "{ zzmod }: zzmod.zzuniq 1\n"),
    ("bash", "a.sh", "#!/bin/bash\ng() {\n  zzuniq \"$1\"\n}\n"),
    ("sequel", "a.sql", "SELECT zzuniq(id) FROM t;\n"),
];

/// Every grammar that declares a call extractor is covered by a snippet.
///
/// The registry is the authority, not this file's memory of it: a grammar
/// linked tomorrow with `CALLS` and no snippet fails here, which is the only
/// thing that keeps the sweep below from becoming "the languages someone
/// remembered".
#[test]
fn the_sweep_covers_every_grammar_that_extracts_calls() {
    let mut uncovered: Vec<&str> = LANGUAGE_SPECS
        .iter()
        .filter(|spec| spec.capabilities.contains(Capability::Calls))
        .map(|spec| spec.grammar)
        .filter(|grammar| !CALL_SNIPPETS.iter().any(|(key, _, _)| key == grammar))
        .collect();
    uncovered.sort_unstable();
    uncovered.dedup();
    assert!(
        uncovered.is_empty(),
        "these grammars declare a call extractor but have no snippet in \
         CALL_SNIPPETS, so the duplicate-callee rule is unmeasured for them: \
         {uncovered:?}"
    );
}

/// ...and in each of them, one call is one reference to its callee.
#[test]
fn one_call_is_one_reference_in_every_language() {
    let mut wrong: Vec<String> = Vec::new();
    for (grammar, path, source) in CALL_SNIPPETS {
        let extraction = extract_file(path, source);
        let all = refs_named(&extraction, "zzuniq");
        // A grammar whose snippet stopped producing a call at all is a broken
        // fixture, and it must not read as a pass.
        let calls = extraction
            .calls
            .iter()
            .filter(|c| c.callee_name == "zzuniq")
            .count();
        if calls != 1 {
            wrong.push(format!(
                "{grammar}: the snippet produced {calls} calls named zzuniq, not 1 \
                 — the fixture stopped exercising a call"
            ));
            continue;
        }
        if all.len() != 1 {
            wrong.push(format!(
                "{grammar}: {} references name zzuniq, not 1: {all:?}",
                all.len()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A chain is one site per link, and none of them eats another's.
#[test]
fn a_long_chain_keeps_one_reference_per_link() {
    let extraction = extract_file(
        "a.rs",
        "fn g(r: H) { r.zza().zzb().zzc().zzd().zze().zzf(); }",
    );
    for link in ["zza", "zzb", "zzc", "zzd", "zze", "zzf"] {
        assert_eq!(
            count(&extraction, link, ReferenceKind::Call),
            1,
            "{link}: one Call reference"
        );
        assert_eq!(
            count(&extraction, link, ReferenceKind::Name),
            0,
            "{link}: no duplicate Name reference"
        );
    }
}

/// A chain whose every link is spelled the same keeps one site per link.
///
/// The rule keys on `(name, end byte)`, and every link here shares the name.
/// If the end byte were dropped from the key — or if the containing-span test
/// reached across links — one call's reference would answer for all six.
#[test]
fn a_chain_of_one_name_is_still_one_site_per_link() {
    let extraction = extract_file("a.rs", "fn g(r: H) { r.zzsame().zzsame().zzsame(); }");
    assert_eq!(
        count(&extraction, "zzsame", ReferenceKind::Call),
        3,
        "three links, three Call references"
    );
    assert_eq!(
        count(&extraction, "zzsame", ReferenceKind::Name),
        0,
        "and no duplicates"
    );
}

/// An argument spelled like the callee it is passed to survives.
///
/// Written with a constant rather than a parameter on purpose: a name the
/// enclosing scope *binds* is suppressed by `name_is_shadowed_by_local`, which
/// is a separate and older rule, and a fixture that tripped over it would pass
/// for the wrong reason. `a_local_argument_was_already_shadowed` pins that
/// older behaviour so the difference stays visible.
#[test]
fn an_argument_spelled_like_its_callee_survives() {
    let extraction = extract_file("a.rs", "fn g() { zzhost(ZZARG, ZZARG); }");
    assert!(
        count(&extraction, "ZZARG", ReferenceKind::Name) >= 1,
        "the argument uses of ZZARG must survive"
    );
}

/// A name the enclosing scope binds has no `Name` reference, and did not
/// before this rule existed either.
///
/// Pinned because it is the fixture trap next to the one above: a reader who
/// sees `fn g(zzarg: H) { zzhost(zzarg) }` produce no reference to `zzarg`
/// would reasonably suspect the duplicate-callee rule, and it is
/// `name_is_shadowed_by_local`.
#[test]
fn a_local_argument_was_already_shadowed() {
    let extraction = extract_file("a.rs", "fn g(zzarg: H) { zzhost(zzarg); }");
    assert_eq!(
        count(&extraction, "zzarg", ReferenceKind::Name),
        0,
        "a local binding suppresses its own mentions — unrelated to callees"
    );
    assert_eq!(
        count(&extraction, "zzhost", ReferenceKind::Call),
        1,
        "and the call itself is still one site"
    );
}

/// A turbofish keeps its duplicate, and that is the deliberate abstention.
///
/// The call records its `generic_function` node, so the callee reference spans
/// `r.zzget::<_, String>` and the name is inside it rather than last. The rule
/// is a suffix test and does not fire.
///
/// This is pinned as a **known cost**, not as desired behaviour. The tier that
/// would take it — "the callee is the only mention of its own name inside the
/// span" — is false for a call recovered from a Rust macro body, where the span
/// is the whole invocation. Measured A/B over one snapshot of this repository:
/// that tier removes 1,164 edges, 1,150 of them genuine duplicates and 14 the
/// only edge their pair has;
/// `a_macro_borne_calls_argument_keeps_its_reference` is the shape of all 14.
/// If this assertion ever starts failing because the duplicate is gone, check
/// that test still passes before celebrating.
#[test]
fn a_turbofish_keeps_its_duplicate_and_that_is_known() {
    let extraction = extract_file("a.rs", "fn g(r: H) { r.zzget::<_, String>(0); }");
    assert_eq!(count(&extraction, "zzget", ReferenceKind::Call), 1);
    assert_eq!(
        count(&extraction, "zzget", ReferenceKind::Name),
        1,
        "the turbofish residue is known and deliberate — see the doc comment"
    );
}

/// A call recovered from inside a Rust macro body keeps its own reference.
///
/// `rust_macro_calls` re-parses the token tree and stamps the *macro
/// invocation's* span on what it finds, because the re-parsed tree has no
/// coordinates in this file. So the callee reference spans the whole
/// `debug_assert!(...)` and the identifier inside it is the **only** evidence
/// the target is reached at all — the macro-borne call itself is frequently
/// unresolvable, because its receiver is a module path.
///
/// Measured A/B over one snapshot of this repository: a rule that dropped "the
/// only mention inside the callee span" removed 14 `References` edges that no
/// `Calls` edge replaced, every one of them this shape.
/// `RpcError::new -> codes::is_reserved_and_undefined` in `devmap-serve` is one
/// of them, and that symbol's only inbound edge is this identifier.
#[test]
fn a_macro_borne_calls_argument_keeps_its_reference() {
    let extraction = extract_file(
        "a.rs",
        "fn g() { debug_assert!(!zzmod::zzcheck(1), \"no\"); }",
    );
    assert!(
        !refs_named(&extraction, "zzcheck").is_empty(),
        "the identifier inside the macro is the only evidence zzcheck is \
         reached; dropping it can report the target dead"
    );
}

/// ...and an ordinary macro-borne method call is not multiplied.
#[test]
fn a_macro_borne_call_is_not_multiplied() {
    let extraction = extract_file("a.rs", "fn g(r: H) { println!(\"{}\", r.zzmacro()); }");
    let total = refs_named(&extraction, "zzmacro").len();
    assert!(
        total >= 1,
        "the macro-borne call must keep at least one reference; got {total}"
    );
    assert!(
        total <= 2,
        "a macro-borne call must not multiply references; got {total}"
    );
}

/// Non-ASCII identifiers are byte spans, and the rule is byte arithmetic.
///
/// Nothing here slices a string, but a rule that did would panic on a
/// multi-byte boundary rather than answer.
#[test]
fn a_multibyte_identifier_is_one_site() {
    let extraction = extract_file("a.rs", "fn g(r: H) { r.zzécho(); r.zzøther(); }");
    for callee in ["zzécho", "zzøther"] {
        assert_eq!(
            count(&extraction, callee, ReferenceKind::Call),
            1,
            "{callee}: one Call reference"
        );
        assert_eq!(
            count(&extraction, callee, ReferenceKind::Name),
            0,
            "{callee}: no duplicate"
        );
    }
}

/// A receiver reached through a path keeps every segment it names but the last.
#[test]
fn a_path_receiver_keeps_its_root() {
    let extraction = extract_file("a.rs", "fn g() { zzroot::zzmid::zzleaf(); }");
    assert_eq!(count(&extraction, "zzleaf", ReferenceKind::Call), 1);
    assert_eq!(count(&extraction, "zzleaf", ReferenceKind::Name), 0);
    assert!(
        count(&extraction, "zzroot", ReferenceKind::Name) >= 1,
        "the path root is a use of zzroot and must survive"
    );
}

/// A call nested inside another call's arguments is its own site.
#[test]
fn a_nested_call_is_its_own_site() {
    let extraction = extract_file("a.rs", "fn g(r: H) { zzouter(r.zzinner(), zzbare()); }");
    for callee in ["zzouter", "zzinner", "zzbare"] {
        assert_eq!(
            count(&extraction, callee, ReferenceKind::Call),
            1,
            "{callee}: one Call reference"
        );
        assert_eq!(
            count(&extraction, callee, ReferenceKind::Name),
            0,
            "{callee}: no duplicate"
        );
    }
}

/// The rule is idempotent and order-independent.
///
/// Extraction of one source twice must give the identical reference list; a
/// rule that mutated a shared map, or that depended on the order references
/// happen to be pushed in, would show here.
#[test]
fn extraction_is_stable_across_repeats() {
    let source = "fn g(r: H) { r.zza().zzb(); zzc(); Zz::zzd(); }";
    let first = extract_file("a.rs", source);
    let second = extract_file("a.rs", source);
    let shape = |e: &Extraction| -> Vec<(String, String, usize, usize)> {
        e.references
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    format!("{:?}", r.kind),
                    r.span.start_byte,
                    r.span.end_byte,
                )
            })
            .collect()
    };
    assert_eq!(shape(&first), shape(&second));
}

/// A file with thousands of calls still answers, and in linear time.
///
/// The rule builds one map over the references and reads it once. A quadratic
/// implementation — a scan of every Call reference per Name reference — is the
/// obvious way to write it and would be invisible on a snippet.
#[test]
fn many_calls_in_one_file_stay_bounded() {
    let mut source = String::from("fn g(r: H) {\n");
    for index in 0..4000 {
        source.push_str(&format!("    r.zzm{index}();\n"));
    }
    source.push_str("}\n");
    let started = std::time::Instant::now();
    let extraction = extract_file("big.rs", &source);
    let elapsed = started.elapsed();
    assert_eq!(
        extraction
            .calls
            .iter()
            .filter(|c| c.callee_name.starts_with("zzm"))
            .count(),
        4000,
        "every call is extracted"
    );
    let duplicates = extraction
        .references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Name && r.name.starts_with("zzm"))
        .count();
    assert_eq!(duplicates, 0, "no duplicate survives at scale");
    // Generous: the bound is here to catch an accidental O(n^2), not to fence
    // the parser's own cost.
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "4000 calls took {elapsed:?}"
    );
}

/// An empty file, and a file with references but no calls, are both no-ops.
///
/// Free names again, for the `name_is_shadowed_by_local` reason above.
#[test]
fn a_file_with_no_calls_keeps_every_reference() {
    let extraction = extract_file("a.rs", "fn g() { let a = ZZONE; let b = ZZTWO; }");
    for name in ["ZZONE", "ZZTWO"] {
        assert!(
            count(&extraction, name, ReferenceKind::Name) >= 1,
            "{name}: a use with no call anywhere must survive"
        );
    }
    let empty = extract_file("empty.rs", "");
    assert!(empty.references.is_empty());
}
