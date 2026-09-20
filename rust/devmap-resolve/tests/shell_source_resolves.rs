//! Shell `source` / `.` resolves to the file it names.
//!
//! `langimports::shell` has extracted `source foo.sh` and `. foo.sh` all along,
//! but `Resolver::resolve_import_path` had no `shell` arm, so every specifier it
//! produced resolved to nothing. Two things followed, and the second is the
//! expensive one:
//!
//!  - The sourced file had no inbound edge, so `unwired_candidates` reported it
//!    as a file nothing depends on. `shell` declares `Capability::Imports`, so
//!    it was not excluded as import-blind the way `.sql` is — the finding was
//!    asserted on the strength of a lookup that had no arm to run.
//!  - The functions the sourced file defines were left to the global tier,
//!    which matches on the name alone. Measured on DevCouncil: `verify.sh`'s
//!    calls to `peak_rss_bytes` fanned out onto **six archived copies** of
//!    `peak_rss.sh` under `benchmarks/results/competition/`, and none of them
//!    onto `rust/tools/peak_rss.sh`, the file it actually sources.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{ResolutionResult, ResolvedEdge};
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

fn edges_into<'a>(result: &'a ResolutionResult, target_file: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| edge.target_file == target_file)
        .collect()
}

/// The helper `rust/tools/peak_rss.sh`, and the two scripts that source it the
/// way this repository really does.
const HELPER: &str = "\
# shellcheck shell=bash
peak_rss_bytes() {
\techo 1
}
";

/// `rust/verify.sh` opens with `cd \"$(dirname \"$0\")\"`, so `tools/peak_rss.sh`
/// is relative to its own directory.
const VERIFY: &str = "\
#!/usr/bin/env bash
. tools/peak_rss.sh
peak_rss_bytes
";

#[test]
fn a_sourced_helper_gets_an_inbound_edge() {
    let result = resolve(&[
        ("rust/tools/peak_rss.sh", HELPER),
        ("rust/verify.sh", VERIFY),
    ]);

    let inbound = edges_into(&result, "rust/tools/peak_rss.sh");
    assert!(
        !inbound.is_empty(),
        "`. tools/peak_rss.sh` names this file; with no inbound edge it is \
         reported as depended on by nothing"
    );
    assert!(
        inbound
            .iter()
            .any(|edge| edge.source_file == "rust/verify.sh"),
        "the edge must come from the script that sources it — got {:?}",
        inbound
            .iter()
            .map(|edge| (&edge.source_file, &edge.target_symbol))
            .collect::<Vec<_>>()
    );
}

/// The specifier is resolved against an ancestor of the sourcing script, so a
/// helper sourced from deeper in the tree finds the same file.
///
/// `rust/tools/memory_model_probe.sh` also writes `. tools/peak_rss.sh`, which
/// is not relative to its own directory but to `rust/`, one level up.
#[test]
fn an_ancestor_relative_specifier_still_finds_it() {
    let probe = "#!/usr/bin/env bash\n. tools/peak_rss.sh\n";
    let result = resolve(&[
        ("rust/tools/peak_rss.sh", HELPER),
        ("rust/tools/memory_model_probe.sh", probe),
    ]);

    assert!(
        edges_into(&result, "rust/tools/peak_rss.sh")
            .iter()
            .any(|edge| edge.source_file == "rust/tools/memory_model_probe.sh"),
        "`tools/peak_rss.sh` from `rust/tools/` resolves one directory up"
    );
}

/// The point of the edge: calls bind to the file that was sourced, not to a
/// same-named copy elsewhere in the corpus.
///
/// This is the half that cost something. Six archived `peak_rss.sh` copies sit
/// under `benchmarks/results/competition/` in this repository, and every
/// `peak_rss_bytes` call in `verify.sh` landed on them.
#[test]
fn the_sourced_copy_wins_over_an_archived_one() {
    let result = resolve(&[
        ("rust/tools/peak_rss.sh", HELPER),
        ("benchmarks/results/20260101/peak_rss.sh", HELPER),
        ("benchmarks/results/20260202/peak_rss.sh", HELPER),
        ("rust/verify.sh", VERIFY),
    ]);

    let called: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_file == "rust/verify.sh" && edge.target_symbol.contains("peak_rss_bytes")
        })
        .map(|edge| edge.target_file.clone())
        .collect();

    assert!(
        !called.is_empty(),
        "`verify.sh` calls `peak_rss_bytes`; some edge must record it"
    );
    assert!(
        called.iter().all(|file| file == "rust/tools/peak_rss.sh"),
        "the sourced file is the one that defines this function here; an \
         archived copy absorbing the call is how a live helper comes to look \
         dead and a stale artifact comes to look used — got {called:?}"
    );
}

/// A specifier naming no indexed file resolves to nothing rather than to the
/// nearest plausible match.
#[test]
fn an_unknown_specifier_resolves_to_nothing() {
    let script = "#!/usr/bin/env bash\n. tools/absent.sh\n";
    let result = resolve(&[
        ("rust/tools/peak_rss.sh", HELPER),
        ("rust/verify.sh", script),
    ]);

    assert!(
        edges_into(&result, "rust/tools/peak_rss.sh")
            .iter()
            .all(|edge| edge.source_file != "rust/verify.sh"),
        "the ancestor walk probes the indexed universe; it must not fall back \
         onto a different file in the directory it was looking in"
    );
}
