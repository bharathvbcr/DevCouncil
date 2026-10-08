//! `xs.push(1)` where no file in the language declares a `push`.
//!
//! `UninferredReceiver` says "the receiver is a value, and naming its owner
//! needs type inference this resolver does not do". That sentence promises
//! there is an owner to name. When no indexed symbol of the caller's language
//! family carries the member's name, there is none: no amount of inference
//! could produce an edge, because every edge ends at an indexed symbol. That
//! is the evidence `NoNamesake` already stands on for a bare name
//! (`bare_name_miss`), and the receiver ladder never asked it.
//!
//! Measured at generation 4144 on this repository: 827 of the 837 JavaScript
//! `uninferred_receiver` rows named a member — `includes`, `push`, `length`,
//! `slice`, `split` — that no JavaScript or TypeScript file declares. Every one
//! sat in the half of the ledger a reader is told may hide an edge, and every
//! one made a walk through its file report itself incomplete.
//!
//! The vetoes are the half worth testing: a namesake in the family keeps the
//! site unattributed, and a namesake in *another* family does not, because a
//! JavaScript call cannot bind to a Rust method.

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
    resolver.resolve_all(&extractions).unwrap()
}

fn classes_for<'a>(
    resolution: &'a ResolutionResult,
    callee: &str,
    receiver: &str,
) -> Vec<&'a UnresolvedClass> {
    resolution
        .unresolved
        .iter()
        .filter(|row| row.callee_name == callee && row.receiver.as_deref() == Some(receiver))
        .map(|row| &row.class)
        .collect()
}

const CALLER: &str = "export function collect(xs) {\n  xs.push(1);\n  return xs.length;\n}\n";

#[test]
fn a_member_no_file_in_the_family_declares_has_no_namesake() {
    let result = resolve(&[("src/collect.js", CALLER)]);
    for member in ["push", "length"] {
        let classes = classes_for(&result, member, "xs");
        assert!(
            !classes.is_empty()
                && classes
                    .iter()
                    .all(|class| matches!(class, UnresolvedClass::NoNamesake)),
            "{member}: {classes:?}"
        );
    }
}

#[test]
fn a_family_namesake_keeps_the_receiver_unattributed() {
    let result = resolve(&[
        ("src/collect.js", CALLER),
        (
            "src/stack.ts",
            "export class Stack {\n  push(x: number) { return x; }\n}\n",
        ),
    ]);
    let classes = classes_for(&result, "push", "xs");
    assert!(
        !classes.is_empty()
            && classes
                .iter()
                .all(|class| matches!(class, UnresolvedClass::UninferredReceiver)),
        "a TypeScript `push` may be the target: {classes:?}"
    );
}

#[test]
fn a_namesake_in_another_family_is_not_a_candidate() {
    let result = resolve(&[
        ("src/collect.js", CALLER),
        (
            "src/stack.rs",
            "pub struct Stack;\nimpl Stack {\n    pub fn push(&mut self, x: u32) -> u32 { x }\n}\n",
        ),
    ]);
    let classes = classes_for(&result, "push", "xs");
    assert!(
        !classes.is_empty()
            && classes
                .iter()
                .all(|class| matches!(class, UnresolvedClass::NoNamesake)),
        "a Rust `push` can never be the target of a JavaScript call: {classes:?}"
    );
}
