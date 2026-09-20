//! A path-qualified Rust call names a module, and `super` inside an inline
//! module names the file it is written in.
//!
//! `rust_module_paths.rs` already covers the `use super::*;` half and the
//! **bare** call that a glob makes possible. The qualified spelling —
//! `mod tests { super::helper() }`, which needs no `use` at all — was covered
//! by neither, and it reached the resolver as the receiver string `"super"`.
//! There it met a guard written for superclass dispatch (`super()` in Python,
//! `base` in C#) and was recorded as having no target.
//!
//! The cost was not a missing edge in the abstract. GitPulse's
//! `src-tauri/src/procguard/mod.rs::run_tree_killer` is called twice through
//! this spelling and executes under coverage (`lcov.info` records `FNDA:1`),
//! and DevMap reported it dead at the **confident** tier — the tier a reader is
//! invited to delete from. Its own repository had already written the finding
//! down as a known false positive.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{ResolutionResult, UnresolvedClass};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn call_edge<'a>(
    result: &'a ResolutionResult,
    callee_suffix: &str,
) -> Option<&'a devmap_resolve::model::ResolvedEdge> {
    result.edges.iter().find(|edge| {
        edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with(callee_suffix)
    })
}

/// The shape this item exists for. The receiver is exactly `super`, the call is
/// inside `mod tests`, and the target is a free function at file level.
///
/// Both spellings are present on purpose. A call written as a macro argument
/// reaches a different extraction arm than a plain one — and `assert_eq!` is
/// where a test module puts most of its calls, so fixing only the plain arm
/// would have left the real corpus untouched.
const QUALIFIED_SUPER_IN_INLINE_MODULE: &str = "\
pub fn helper(value: u32) -> u32 {
    value + 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_adds() {
        assert_eq!(super::helper(1), 2);
    }

    #[test]
    fn it_adds_without_a_macro() {
        let plain = super::helper(1);
        assert!(plain == 2);
    }
}
";

/// The extractor half, asserted on its own so a failure names which side broke.
#[test]
fn the_extractor_absorbs_an_inline_modules_super_receiver() {
    let extraction = extract_file("crates/thing/src/work.rs", QUALIFIED_SUPER_IN_INLINE_MODULE);
    let receivers: Vec<Option<&str>> = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "helper")
        .map(|call| call.receiver_expr.as_deref())
        .collect();
    assert_eq!(
        receivers.len(),
        2,
        "both the macro-argument call and the plain one are extracted; got {receivers:?}"
    );
    assert!(
        receivers.iter().all(|receiver| *receiver == Some("self")),
        "the `super` is spent on the inline module, so what remains is this \
         file's own module — in both extraction arms; got {receivers:?}"
    );
}

#[test]
fn a_qualified_super_call_in_an_inline_module_is_a_same_file_edge() {
    let (_, result) = resolve(&[("crates/thing/src/work.rs", QUALIFIED_SUPER_IN_INLINE_MODULE)]);

    let edge = call_edge(&result, "::helper").expect(
        "`super::helper(1)` inside `mod tests` names this file's `helper`; \
         without this edge the function has no inbound evidence and the \
         dead-code pass reports it at the confident tier",
    );
    assert_eq!(edge.source_file, "crates/thing/src/work.rs");
    assert_eq!(edge.target_file, "crates/thing/src/work.rs");
    assert_eq!(
        format!("{:?}", edge.evidence.expect("resolver evidence").kind),
        "SameFile",
        "the declaration is in this very file"
    );
}

/// The regression this fix must not cause. At **file level** the same text
/// names the parent directory's module, which is a different file — binding it
/// to this file's own declaration would fabricate an edge.
const QUALIFIED_SUPER_AT_FILE_LEVEL: &str = "\
pub fn helper(value: u32) -> u32 {
    value + 1
}

pub fn run() -> u32 {
    super::helper(1)
}
";

#[test]
fn a_qualified_super_call_at_file_level_does_not_bind_to_this_file() {
    let (_, result) = resolve(&[("src/deep/leaf.rs", QUALIFIED_SUPER_AT_FILE_LEVEL)]);

    let bogus = result.edges.iter().find(|edge| {
        edge.edge_kind == EdgeKind::Calls
            && edge.source_file == "src/deep/leaf.rs"
            && edge.target_symbol.ends_with("::helper")
    });
    assert!(
        bogus.is_none(),
        "`super::helper()` written at file level is `deep::helper`, not this \
         file's `helper`; binding it here invents a call the code does not \
         make. got {bogus:?}"
    );
}

/// And the row it leaves behind must state a reason that is true. Rust has no
/// inheritance, so "superclass dispatch" was never an accurate account of a
/// `super::` path. `ModulePath` is the tier whose own documentation names it.
#[test]
fn an_unbound_rust_super_path_is_classified_as_a_module_path() {
    let (_, result) = resolve(&[("src/deep/leaf.rs", QUALIFIED_SUPER_AT_FILE_LEVEL)]);

    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "helper")
        .collect();
    assert!(!rows.is_empty(), "the unbound call is still recorded");
    assert!(
        rows.iter()
            .all(|row| row.class == UnresolvedClass::ModulePath),
        "a Rust `super::` path is a module path, not superclass dispatch; \
         got {:?}",
        rows.iter().map(|row| row.class.label()).collect::<Vec<_>>()
    );
}

/// `self::` is the same fact written without the nesting, and must bind for the
/// same reason.
#[test]
fn a_self_qualified_call_binds_to_this_files_function() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "pub fn helper() -> u32 {\n    1\n}\n\npub fn run() -> u32 {\n    self::helper()\n}\n",
    )]);

    let edge = call_edge(&result, "::helper").expect("`self::helper()` names this file's helper");
    assert_eq!(
        format!("{:?}", edge.evidence.expect("resolver evidence").kind),
        "SameFile"
    );
}

/// The guard that the new rung must not breach: a *method* call through a
/// value receiver is unaffected, and a superclass `super` in a language that
/// has inheritance keeps abstaining.
#[test]
fn python_superclass_dispatch_still_abstains() {
    let (_, result) = resolve(&[(
        "src/app.py",
        "def helper():\n    return 1\n\n\nclass C(Base):\n    def run(self):\n        return super().helper()\n",
    )]);

    let bogus = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("::helper"));
    assert!(
        bogus.is_none(),
        "`super().helper()` names a base class declaration, not the module's \
         free function; got {bogus:?}"
    );
}

/// A receiver with segments left over is deliberately left alone — the rewrite
/// claims only what it can prove.
#[test]
fn a_multi_segment_super_receiver_is_not_rewritten() {
    let extraction = extract_file(
        "crates/thing/src/work.rs",
        "pub struct Thing;\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        let _ = super::Thing::build();\n    }\n}\n",
    );
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "build")
        .expect("the call is extracted");
    assert_eq!(
        call.receiver_expr.as_deref(),
        Some("super::Thing"),
        "only a receiver of exactly `super` is absorbed; anything further is \
         left for a rung that can place it"
    );
}
