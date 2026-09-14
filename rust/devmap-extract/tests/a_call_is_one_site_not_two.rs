//! A call is one attribution site, in every language that has a call extractor.
//!
//! Every call extractor records a `Call` reference naming the callee. The
//! generic identifier walker then visits the same method identifier, and
//! `is_call_callee` is what is supposed to stop it recording a *second*
//! reference for it as an ordinary `Name` use.
//!
//! That suppression had drifted out of step with the facts it depends on. Three
//! functions each carry their own copy of "which grammars spell a name reached
//! through something else": `member_access_fields` lists four spellings,
//! `member_access_receiver` five, and `is_call_callee` three. The resolver
//! records a `Name` reference's failure exactly when `member_access_receiver`
//! gave it a receiver — so every spelling in the five-entry list and missing
//! from the three-entry one produced a duplicate row, and the two missing
//! entries are Rust's and Scala's `field_expression` and Rust's
//! `scoped_identifier`.
//!
//! Run against the code this replaces, the matrix below fails for **eleven** of
//! its seventeen languages. On the DevCouncil repository the duplicate cost
//! Rust **49,989** of its 150,163 attribution sites — 27% of the corpus total
//! of 182,181 — and it is filed as a *failure* even where the call itself
//! resolved: a `Widget::omegafn()` that produced a `Calls` edge at 1.0
//! confidence also produced an `uninferred_receiver` row claiming it had not.
//!
//! The matrix below is the gate. It is a *matrix* and not three cases because
//! the defect is one rule missing grammar spellings: fixing the spellings that
//! happened to be measured is how the three-entry list got three entries.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind};

fn extract(path: &str, source: &str) -> Extraction {
    extract_file(path, source)
}

/// How many references of each family name `callee`.
fn tally(extraction: &Extraction, callee: &str) -> (usize, usize) {
    let calls = extraction
        .references
        .iter()
        .filter(|r| {
            r.name == callee && matches!(r.kind, ReferenceKind::Call | ReferenceKind::Constructor)
        })
        .count();
    let names = extraction
        .references
        .iter()
        .filter(|r| r.name == callee && r.kind == ReferenceKind::Name)
        .count();
    (calls, names)
}

/// One row per language: the file, its source, and the callee names a method
/// call and a path/static call spell in it.
///
/// `None` for the path call where the language has no second spelling worth
/// distinguishing (or spells it the same way).
struct Case {
    path: &'static str,
    source: &'static str,
    method_call: &'static str,
    path_call: Option<&'static str>,
    /// The receiver's own name, which must keep its `Name` reference: it is the
    /// only evidence that symbol is alive, and refusing it is what made every
    /// method receiver invisible to the graph once before.
    receiver: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        path: "a.rs",
        source: "fn d(r: H) { r.zzmm(); Zzrecv::zzpp(); }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.scala",
        source: "object D { def g(r: H): Unit = { r.zzmm(); Zzrecv.zzpp() } }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.sol",
        source: "contract D { function g(H r) public { r.zzmm(); Zzrecv.zzpp(); } }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.java",
        source: "class D { void g(H r) { r.zzmm(); Zzrecv.zzpp(); } }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.kt",
        source: "fun g(r: H) { r.zzmm(); Zzrecv.zzpp() }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.rb",
        source: "def g(r)\n  r.zzmm\n  Zzrecv.zzpp\nend\n",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        // `None`, and it is a **pre-existing** gap rather than this change's:
        // Ruby spells a constant receiver as a `constant` node, which is not in
        // the identifier walker's kind list, so `Zzrecv` had no `Name`
        // reference before the duplicate was dropped either. Recorded here
        // rather than asserted away — the suffix rule cannot have taken it
        // (`Zzrecv` ends four bytes left of the callee span), and claiming the
        // receiver survives would be a claim this fixture does not test.
        receiver: None,
    },
    Case {
        path: "a.swift",
        source: "func g(r: H) { r.zzmm(); Zzrecv.zzpp() }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.c",
        source: "void g(H r, H *p) { r.zzmm(); p->zzpp(); }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: None,
    },
    Case {
        path: "a.cpp",
        source: "void g(H r) { r.zzmm(); Zzrecv::zzpp(); }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: None,
    },
    Case {
        path: "a.lua",
        source: "local function g(r)\n  r:zzmm()\n  r.zzpp()\nend\n",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: None,
    },
    Case {
        path: "a.r",
        source: "g <- function(r) {\n  r$zzmm()\n  Zzrecv::zzpp()\n}\n",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    // The seven that were already correct, kept as the negative control: a fix
    // to the thirteen must not start dropping *their* references.
    Case {
        path: "a.go",
        source: "package p\nfunc G(r H) { r.Zzmm(); zzrecv.Zzpp() }",
        method_call: "Zzmm",
        path_call: Some("Zzpp"),
        receiver: Some("zzrecv"),
    },
    Case {
        path: "a.py",
        source: "def g(r):\n    r.zzmm()\n    Zzrecv.zzpp()\n",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.ts",
        source: "export function g(r: H) { r.zzmm(); Zzrecv.zzpp(); }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.cs",
        source: "class D { void G(H r) { r.Zzmm(); Zzrecv.Zzpp(); } }",
        method_call: "Zzmm",
        path_call: Some("Zzpp"),
        receiver: Some("Zzrecv"),
    },
    Case {
        path: "a.php",
        source: "<?php function g($r) { $r->zzmm(); Zzrecv::zzpp(); }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: None,
    },
    Case {
        path: "a.dart",
        source: "void g(H r) { r.zzmm(); Zzrecv.zzpp(); }",
        method_call: "zzmm",
        path_call: Some("zzpp"),
        receiver: Some("Zzrecv"),
    },
];

/// A call names its callee once, not twice, in every linked language.
#[test]
fn a_callee_is_not_also_a_name_reference() {
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        let extraction = extract(case.path, case.source);
        for callee in [Some(case.method_call), case.path_call]
            .into_iter()
            .flatten()
        {
            let (calls, names) = tally(&extraction, callee);
            if calls == 0 {
                broken.push(format!(
                    "{}: no Call reference for {callee:?} at all — the fixture \
                     stopped exercising a call, not the suppression",
                    case.path
                ));
                continue;
            }
            if names != 0 {
                broken.push(format!(
                    "{}: {callee:?} has {calls} Call reference(s) and {names} \
                     duplicate Name reference(s)",
                    case.path
                ));
            }
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// ...and the receiver keeps its own reference.
///
/// The property the three-entry list was written to protect: refusing both
/// halves of a member access made every method receiver invisible, and a
/// module-level singleton used the way singletons are used read as dead in
/// eleven files at once. Dropping the duplicate must not cost this.
#[test]
fn the_receiver_half_keeps_its_reference() {
    let mut missing: Vec<String> = Vec::new();
    for case in CASES {
        let Some(receiver) = case.receiver else {
            continue;
        };
        let extraction = extract(case.path, case.source);
        let seen = extraction
            .references
            .iter()
            .any(|r| r.name == receiver && r.kind == ReferenceKind::Name);
        if !seen {
            missing.push(format!(
                "{}: receiver {receiver:?} has no Name reference",
                case.path
            ));
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

/// A receiver spelled exactly like the method it calls keeps its reference.
///
/// This is why the rule is a *suffix* test and not containment. In `zz::zz()`
/// both halves are spelled `zz` and both lie inside the callee expression
/// `zz::zz`; only the right-hand one ends where the call's reference does, so
/// only it is the callee's second copy.
#[test]
fn a_receiver_named_like_its_method_is_kept() {
    let extraction = extract("a.rs", "fn d() { zzsame::zzsame(); }");
    let (calls, names) = tally(&extraction, "zzsame");
    assert_eq!(calls, 1, "one Call reference for the callee");
    assert_eq!(
        names, 1,
        "the path root is a use of `zzsame` in its own right and must survive; \
         got {names} Name reference(s)"
    );
    // ...and the one that survived is the left half, not the right.
    let name_span = extraction
        .references
        .iter()
        .find(|r| r.name == "zzsame" && r.kind == ReferenceKind::Name)
        .map(|r| (r.span.start_byte, r.span.end_byte))
        .expect("the surviving Name reference");
    let call_span = extraction
        .references
        .iter()
        .find(|r| r.name == "zzsame" && r.kind == ReferenceKind::Call)
        .map(|r| (r.span.start_byte, r.span.end_byte))
        .expect("the Call reference");
    assert!(
        name_span.1 < call_span.1,
        "the surviving Name reference must be the path root at {name_span:?}, \
         left of the callee span end {}",
        call_span.1
    );
}

/// An argument that happens to share the callee's name is not a duplicate.
///
/// The call reference spans the callee expression only, so an argument is
/// outside it — but a rule that tested containment against the *call* node, or
/// that ignored the name, would take this one.
#[test]
fn an_argument_sharing_the_callee_name_is_kept() {
    let extraction = extract("a.py", "def g(zzarg):\n    zzarg(zzarg)\n");
    let names = extraction
        .references
        .iter()
        .filter(|r| r.name == "zzarg" && r.kind == ReferenceKind::Name)
        .count();
    assert!(
        names >= 1,
        "the argument use of `zzarg` must survive; {names} Name reference(s) left"
    );
}

/// Chained calls each keep exactly one site.
#[test]
fn a_chain_is_one_site_per_link() {
    let extraction = extract("a.rs", "fn d(r: H) { r.zzone().zztwo().zzthree(); }");
    for callee in ["zzone", "zztwo", "zzthree"] {
        let (calls, names) = tally(&extraction, callee);
        assert_eq!(calls, 1, "{callee}: one Call reference");
        assert_eq!(names, 0, "{callee}: no duplicate Name reference");
    }
}

/// The rule must not fire across two different sites that merely share a name.
///
/// Two calls to the same method in one function are two sites, and a map keyed
/// only by name — without the end byte — would collapse them.
#[test]
fn two_calls_to_one_method_are_two_sites() {
    let extraction = extract("a.rs", "fn d(r: H, s: H) { r.zzdup(); s.zzdup(); }");
    let (calls, names) = tally(&extraction, "zzdup");
    assert_eq!(calls, 2, "two Call references, one per site");
    assert_eq!(names, 0, "and no duplicate Name reference for either");
}
