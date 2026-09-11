//! X48 — a receiver rooted at a host global **object** is the runtime's, not a
//! value whose type could not be inferred.
//!
//! `UnresolvedClass::UninferredReceiver` is documented as "a method call whose
//! receiver exists but could not be typed" — a structural limit of a
//! syntax-directed extractor, and the one class of miss no extractor work will
//! remove. `console.log(...)` is not that. `console` is a property of the
//! global object; its type is stated by the runtime, exactly as `std::fs`'s is
//! stated by Rust's extern prelude, which X43 already acted on.
//!
//! The distinction is not cosmetic. `net_permille` is
//! `resolved / (resolved + unresolved - explained)`, and `HostGlobal` is one of
//! the three explained classes while `UninferredReceiver` is not — so every
//! `JSON.stringify` sat in the denominator of the number this project ratchets,
//! permanently, with no work that could ever move it.
//!
//! Measured at 4cfec79: **126 of the 316** JavaScript `uninferred_receiver`
//! rows on this repository were rooted at one of these names, and 7,408 of
//! 58,904 across scholarlm's JS/TS.
//!
//! The table is never sufficient by itself, and the three guards are what these
//! tests are mostly about: a scope that binds the name, a corpus that declares
//! it, and a receiver that is an expression rather than a path all keep their
//! rows where they were.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{ResolutionResult, UnresolvedClass};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions)
}

/// The class of every unresolved row for `callee_name`.
fn classes_for(result: &ResolutionResult, name: &str) -> Vec<String> {
    result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == name)
        .map(|row| row.class.label().to_string())
        .collect()
}

fn environment_for(result: &ResolutionResult, name: &str) -> Option<String> {
    result.unresolved.iter().find_map(|row| {
        (row.callee_name == name)
            .then(|| match &row.class {
                UnresolvedClass::HostGlobal { environment } => Some(environment.clone()),
                _ => None,
            })
            .flatten()
    })
}

const RUNTIME_USER: &str = "\
export function report(rows) {
  console.log(JSON.stringify(rows));
  const cwd = process.env.PWD;
  return Math.max(rows.length, 0) + cwd.length;
}
";

#[test]
fn a_member_of_a_runtime_object_is_explained_by_the_runtime() {
    let result = resolve(&[("report.js", RUNTIME_USER)]);

    for name in ["log", "stringify", "max"] {
        let classes = classes_for(&result, name);
        assert!(
            !classes.is_empty(),
            "{name} must still be recorded — the ledger covers every rung that ran"
        );
        assert!(
            classes.iter().all(|class| class == "host_global"),
            "{name} is a member of an object the runtime injects, not a value \
             whose type could not be inferred — got {classes:?}"
        );
    }
    assert_eq!(
        environment_for(&result, "log").as_deref(),
        Some("web+node"),
        "the environment is the evidence and travels with the classification"
    );
    assert_eq!(
        environment_for(&result, "stringify").as_deref(),
        Some("ecma")
    );
}

/// The abstention this rung is bounded by, asserted so it cannot be widened by
/// accident.
///
/// `process.env.PWD` reads a property of a property, and `process` is what the
/// runtime injected either way — so the *fact* is that this is the runtime's.
/// The rung still declines it, because X44 reduces a receiver structurally and
/// `JSON.stringify(rows)` as the receiver of `.padStart(…)` is recorded as
/// `JSON.stringify`: after that reduction a call result and a property read are
/// the same string, and claiming one would claim the other. Requiring the
/// receiver to *be* the root costs these rows and invents nothing, which is the
/// trade this ladder makes everywhere else.
#[test]
fn a_property_path_is_declined_because_a_reduced_receiver_cannot_prove_it() {
    let result = resolve(&[("report.js", RUNTIME_USER)]);

    let classes = classes_for(&result, "PWD");
    assert!(
        !classes.is_empty(),
        "`process.env.PWD` is still recorded — the ledger covers every rung that ran"
    );
    assert!(
        classes.iter().all(|class| class != "host_global"),
        "the rung must not claim a receiver it cannot tell from a call result — \
         got {classes:?}"
    );
}

/// The corpus veto, and the half that matters. A repository that declares its
/// own `Date` is a repository where `Date.parse` may well name that
/// declaration, and calling it "the runtime declares this" would hide exactly
/// the defect this tier exists to surface.
#[test]
fn a_runtime_name_the_corpus_declares_stays_a_defect() {
    const OWN_DATE: &str = "\
export class Date {
  static parse(text) {
    return text;
  }
}
";
    const USER: &str = "\
export function when(text) {
  return Date.parse(text);
}
";
    let result = resolve(&[("date.js", OWN_DATE), ("when.js", USER)]);

    let classes = classes_for(&result, "parse");
    assert!(
        classes.iter().all(|class| class != "host_global"),
        "this corpus declares its own `Date`; the runtime's is not the evidence \
         — got {classes:?}"
    );
}

/// The scope veto. `const process = spawn(...)` makes `process.kill()` a call
/// on a local value, and the runtime's `process` says nothing about it.
#[test]
fn a_scope_that_binds_the_name_keeps_the_receiver_local() {
    const SHADOWED: &str = "\
export function run(spawn) {
  const process = spawn('ls');
  return process.kill();
}
";
    let result = resolve(&[("run.js", SHADOWED)]);

    let classes = classes_for(&result, "kill");
    assert!(
        classes.iter().all(|class| class != "host_global"),
        "`process` here is what this function bound, not what the runtime \
         injected — got {classes:?}"
    );
}

/// The import veto, which needs no rung of its own: an import of the name
/// returns `External` several rungs earlier, and that is the stronger answer.
#[test]
fn an_imported_name_is_external_and_never_reaches_the_object_table() {
    const IMPORTED: &str = "\
import { performance } from 'perf_hooks';

export function tick() {
  return performance.now();
}
";
    let result = resolve(&[("tick.js", IMPORTED)]);

    let classes = classes_for(&result, "now");
    assert!(
        classes.iter().all(|class| class != "host_global"),
        "an import statement in this file is stronger evidence than any name \
         list — got {classes:?}"
    );
}

/// A **callee** that is one of these names is not a call the runtime can
/// answer: `console(...)` is an extraction defect and must keep the tier that
/// says so. The object table is asked only about receivers.
#[test]
fn the_object_table_never_answers_for_a_bare_callee() {
    const CALLED_AS_FUNCTION: &str = "\
export function odd() {
  return console('what');
}
";
    let result = resolve(&[("odd.js", CALLED_AS_FUNCTION)]);

    let classes = classes_for(&result, "console");
    assert!(
        classes.iter().all(|class| class != "host_global"),
        "a call whose callee is `console` cannot be a real call; classifying it \
         as expected would hide the extraction defect — got {classes:?}"
    );
}

/// A chained call whose text happens to start at a runtime object is an
/// expression, not a path. `JSON.stringify(rows).length` reads a property of a
/// *string*, and that string's type is what could not be inferred.
#[test]
fn a_chained_call_rooted_at_a_runtime_object_is_still_an_uninferred_receiver() {
    const CHAINED: &str = "\
export function size(rows) {
  return JSON.stringify(rows).padStart(4, '0');
}
";
    let result = resolve(&[("size.js", CHAINED)]);

    let classes = classes_for(&result, "padStart");
    assert!(
        classes.iter().all(|class| class != "host_global"),
        "the receiver of `padStart` is the string `stringify` returned, whose \
         type is exactly what this resolver cannot infer — got {classes:?}"
    );
}

/// No other language family has a global object for these names to live on. A
/// Python `math.floor(x)` is answered by the import rungs, and must not be
/// answered from a JavaScript table.
#[test]
fn the_object_table_is_javascript_only() {
    const PYTHON: &str = "\
import math


def area(radius):
    return math.floor(radius)
";
    let result = resolve(&[("area.py", PYTHON)]);

    let classes = classes_for(&result, "floor");
    assert!(
        classes.iter().all(|class| class != "host_global"),
        "`math` in Python is an import, and the import rungs own it — got \
         {classes:?}"
    );
}
