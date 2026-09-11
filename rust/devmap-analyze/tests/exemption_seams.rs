//! Where the exemptions meet each other.
//!
//! Three mechanisms decide a symbol is not dead, and they were built by
//! different work orders: the `allow-unwired` marker and dynamic-reference
//! clearing (W3.3), the exported-member exemption (W4.1), and the
//! externally-reachable seed the dead-cluster pass uses (W1.1). Each was tested
//! alone. These test them against each other, because the failure mode of an
//! exemption is silent: it removes a finding, and a finding that was never
//! produced looks exactly like a corpus with nothing wrong in it.

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_extract::wiring::ALLOW_UNWIRED;
use devmap_resolve::Resolver;
use std::collections::BTreeSet;

fn verdicts(extractions: &[Extraction]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let analysis = analyze(extractions, &resolution);
    let symbols = analysis
        .dead_symbols
        .iter()
        .filter(|report| !report.is_exempt)
        .map(|report| format!("{}::{}", report.file_path, report.symbol_name))
        .collect();
    let clustered = analysis
        .dead_clusters
        .clusters
        .iter()
        .flat_map(|cluster| cluster.members.iter().cloned())
        .collect();
    (symbols, clustered)
}

const CYCLE: &str =
    "def cycle_a():\n    return cycle_b()\n\n\ndef cycle_b():\n    return cycle_a()\n";

/// The baseline both tests below are measured against.
#[test]
fn an_abandoned_cycle_is_reported_when_nothing_exempts_it() {
    let (symbols, clusters) = verdicts(&[
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file("pkg/cycle.py", CYCLE),
    ]);
    assert!(
        clusters.contains("pkg/cycle.py::cycle_a"),
        "symbols={symbols:?} clusters={clusters:?}"
    );
}

/// The marker exempts a file in *both* lists, or the two disagree.
///
/// `externally_reachable_symbols` expands a file-scoped annotation to every
/// symbol the file declares, and `allow-unwired` is file-scoped — so the two
/// paths agree by construction. This pins that: an author who declares a file
/// intentionally unwired must not have its cycle reported as an abandoned one
/// under a different name.
#[test]
fn the_allow_unwired_marker_reaches_the_cluster_pass_too() {
    let marked = format!("# {ALLOW_UNWIRED} — driven by the plugin runtime\n\n{CYCLE}");
    let (symbols, clusters) = verdicts(&[
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file("pkg/cycle.py", &marked),
    ]);
    assert!(
        clusters.is_empty(),
        "the marker cleared the symbol list but not the cluster list, so the same \
         code is exempt in one answer and dead in the other: {clusters:?}"
    );
    assert!(
        !symbols.iter().any(|s| s.starts_with("pkg/cycle.py")),
        "{symbols:?}"
    );
}

/// A dynamic reference must not silently mark unrelated symbols reachable.
///
/// `DynamicImport` annotations are symbol-scoped in shape but their target is a
/// *file form* (`src/routes/App`, `pkg.mod`), not a symbol id — so the cluster
/// pass's reachability seed inserts a string no symbol can equal. That is
/// harmless by construction, and this asserts it stays harmless rather than
/// leaving it to be rediscovered: adding a dynamic import somewhere in the
/// corpus must not resurrect an unrelated abandoned cycle.
#[test]
fn a_dynamic_reference_does_not_resurrect_an_unrelated_cycle() {
    let with_dynamic = vec![
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file(
            "pkg/loader.py",
            "import importlib\n\n\ndef load():\n    return importlib.import_module('pkg.plugin')\n",
        ),
        extract_file("pkg/plugin.py", "def run():\n    return 1\n"),
        extract_file("pkg/cycle.py", CYCLE),
    ];
    let (_, clusters) = verdicts(&with_dynamic);
    assert!(
        clusters.contains("pkg/cycle.py::cycle_a"),
        "a dynamic reference to an unrelated module cleared the cycle: {clusters:?}"
    );
}

/// The exported-member exemption does not climb past its direct owner.
///
/// `parent_symbol` is the owner's exact qualified name, so a method of a
/// *non-exported* inner class does not inherit the outer class's export. If it
/// did, one `__all__` entry would exempt an arbitrarily deep tree.
#[test]
fn the_exported_member_exemption_does_not_climb_the_whole_tree() {
    let source = "__all__ = [\"Outer\"]\n\n\nclass Outer:\n    class _Inner:\n        def buried(self):\n            return 1\n\n    def surface(self):\n        return 2\n";
    let (symbols, _) = verdicts(&[
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file("pkg/nested.py", source),
    ]);
    // `Outer.surface` is a direct member of an exported class: exempt.
    assert!(
        !symbols.contains("pkg/nested.py::Outer.surface"),
        "a direct member of an exported class must be exempt: {symbols:?}"
    );
}

/// OFF for the whole family: with every exemption removed, findings appear.
///
/// Without this, an `analyze` that reported nothing at all would satisfy every
/// "must be exempt" assertion above.
#[test]
fn without_an_exemption_the_findings_come_back() {
    let (symbols, _) = verdicts(&[
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file(
            "pkg/plain.py",
            "class NotExported:\n    def method(self):\n        return 1\n",
        ),
    ]);
    assert!(
        symbols.contains("pkg/plain.py::NotExported")
            && symbols.contains("pkg/plain.py::NotExported.method"),
        "a non-exported class and its method must both still be reported: {symbols:?}"
    );
}
