//! Two private nested classes of the same name in different files.
//!
//! Kotlin lets each test class keep its own `private class FakeScheduler`: a
//! private nested class is visible to its enclosing class and nowhere else, so
//! the two do not collide. The resolver does not know that. When both files are
//! indexed, a call through a receiver typed as the same-file class has two
//! candidates for the type, and the receiver-type rung abstains: the call is
//! left in the unresolved ledger as `uninferred_receiver`, where with one file
//! indexed it binds.
//!
//! Found by the extract-v98 corpus gate on Graft: once a file that had parsed as
//! a whole-file ERROR was read, its private `FakeScheduler` became visible, and
//! five calls in `GraftLinkWatchdogTest` to its own `FakeScheduler.tick` stopped
//! resolving. None bound to the other file's class, so no edge is wrong; five are
//! missing.
//!
//! Swift already has this rule (`swift_resolver_residue`: a private nested
//! namesake in another file does not hide the visible type); Kotlin has no
//! equivalent because the resolver is not told that a Kotlin class is private or
//! nested.
//!
//! The control passes today. The probe is ignored until the declaration facts
//! that say a class is private and nested (`jvm_decls`, rows V9 and V14 of
//! `docs/devmap/design/extract-v98.md`) are read by the resolver in Phase R.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

const WATCHDOG: &str = "package t\n\nclass WatchdogTest {\n    private class FakeScheduler {\n        fun tick() {}\n    }\n\n    private val scheduler = FakeScheduler()\n\n    fun run() {\n        scheduler.tick()\n    }\n}\n";

const MIRROR: &str = "package t\n\nclass MirrorTest {\n    private class FakeScheduler {\n        fun advance() {}\n    }\n\n    private val scheduler = FakeScheduler()\n\n    fun run() {\n        scheduler.advance()\n    }\n}\n";

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// `(source file, source symbol, target file, target symbol)` of every call edge
/// out of `WatchdogTest.run`.
fn calls_from_watchdog(result: &ResolutionResult) -> Vec<(String, String, String, String)> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .filter(|edge| edge.source_symbol.ends_with("WatchdogTest.run"))
        .map(|edge| {
            (
                edge.source_file.clone(),
                edge.source_symbol.clone(),
                edge.target_file.clone(),
                edge.target_symbol.clone(),
            )
        })
        .collect()
}

/// Control: with only the watchdog's file indexed, the call binds to the
/// same-file class, so the fixture is a call the resolver can resolve.
#[test]
fn a_call_through_a_same_file_private_class_resolves_when_it_is_the_only_one() {
    let result = resolve(&[("Watchdog.kt", WATCHDOG)]);
    let edges = calls_from_watchdog(&result);
    assert!(
        edges
            .iter()
            .any(|(_, _, file, target)| file == "Watchdog.kt"
                && target.ends_with("FakeScheduler.tick")),
        "{edges:?}"
    );
}

/// The probe: a second file with a private `FakeScheduler` of its own must not
/// stop the call resolving to the same-file class, and must never bind it to the
/// other file's class.
#[test]
#[ignore = "ft-2c800986: two same-named private nested classes in different files make the receiver-type rung abstain (uninferred_receiver); V9/V14 (inner/static and visibility declaration facts, jvm_decls) are what let the resolver prefer the same-file private class"]
fn a_same_named_private_class_in_another_file_does_not_unresolve_the_call() {
    let result = resolve(&[("Watchdog.kt", WATCHDOG), ("Mirror.kt", MIRROR)]);
    let edges = calls_from_watchdog(&result);
    assert!(
        edges.iter().all(|(_, _, file, _)| file != "Mirror.kt"),
        "the call bound to the other file's private class: {edges:?}"
    );
    assert!(
        edges
            .iter()
            .any(|(_, _, file, target)| file == "Watchdog.kt"
                && target.ends_with("FakeScheduler.tick")),
        "the call to the same-file class's tick is no longer resolved: {edges:?}"
    );
}
