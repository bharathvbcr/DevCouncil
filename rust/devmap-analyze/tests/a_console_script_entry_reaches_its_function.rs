//! A console-script declaration reaches the function it names.
//!
//! `[project.scripts] fxtool = "pkg.cli:main_entry"` is a call site. `pip`
//! writes a launcher that imports `pkg.cli` and calls `main_entry`, and that
//! launcher is generated at install time and lives outside every corpus — so
//! nothing in the repository need ever reference the function.
//!
//! The kernel read half of that declaration. `config_script_entry`
//! (`devmap-extract/src/wiring.rs`) marked the manifest itself a
//! `WiringKind::ScriptEntry`, and `ScriptEntry` exempts only symbols whose
//! `target_symbol` is that same file — a TOML file, which declares none. The
//! `module:attr` target was never resolved, so the entry function was reported
//! as a dead symbol. Measured on the fixture below with the pre-fix release
//! kernel:
//!
//! ```text
//!   devmap --json dead
//!   {"file_path":"pkg/cli.py","symbol_name":"main_entry",
//!    "confidence":0.8999999761581417,"is_exempt":false,"exemption_reason":null}
//! ```
//!
//! 0.9 is what `code_graph.rs::confidence_label` renders `extracted` — the tier
//! `CLAUDE.md` tells agents to act on. The map proposed deleting the program's
//! entry point, confidently.
//!
//! Every test here asserts **both** directions. An exemption that is always on
//! is worth exactly as little as one that never fires, and a suite that only
//! pins the cleared case passes against an `analyze` that reports nothing.

use devmap_analyze::{analyze, exempt_symbol_names};
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, WiringKind};
use devmap_resolve::Resolver;

const CLI: &str = "\
def main_entry():
    return helper()


def helper():
    return 1


def stranded():
    return 2
";

/// `main_entry` is named by the manifest, `helper` is called by `main_entry`,
/// and `stranded` is named by nothing. The third is the control: it shares a
/// file with the entry point and must stay dead, or the exemption is
/// file-scoped and has simply reproduced the `ScriptEntry` over-exemption one
/// level down.
fn fixture(manifest: &str) -> Vec<Extraction> {
    vec![
        extract_file("pyproject.toml", manifest),
        extract_file("pkg/__init__.py", ""),
        extract_file("pkg/cli.py", CLI),
    ]
}

/// `(file_path, symbol_name, exemption_reason)` for every symbol `analyze`
/// reports on, exempt or not.
fn verdicts(extractions: &[Extraction]) -> Vec<(String, String, bool, Option<String>)> {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    analyze(extractions, &resolution)
        .dead_symbols
        .iter()
        .map(|report| {
            (
                report.file_path.clone(),
                report.symbol_name.clone(),
                report.is_exempt,
                report.exemption_reason.clone(),
            )
        })
        .collect()
}

fn verdict_for<'a>(
    reports: &'a [(String, String, bool, Option<String>)],
    file: &str,
    symbol: &str,
) -> Option<&'a (String, String, bool, Option<String>)> {
    reports
        .iter()
        .find(|(path, name, _, _)| path == file && name == symbol)
}

const FLAT_MANIFEST: &str = "\
[project]
name = \"fx\"
version = \"0.1.0\"

[project.scripts]
fxtool = \"pkg.cli:main_entry\"
";

#[test]
fn the_entry_function_is_exempt_and_says_why() {
    let reports = verdicts(&fixture(FLAT_MANIFEST));
    let entry = verdict_for(&reports, "pkg/cli.py", "main_entry").unwrap_or_else(|| {
        panic!("main_entry is not in the dead-symbol report at all: {reports:?}")
    });
    assert!(
        entry.2,
        "`[project.scripts] fxtool = \"pkg.cli:main_entry\"` is the call site, \
         and the map still proposes deleting the function it names: {entry:?}"
    );
    let reason = entry
        .3
        .as_deref()
        .expect("an exempt symbol must carry the reason it was cleared");
    assert!(
        reason.contains("pyproject.toml") && reason.contains("pkg.cli:main_entry"),
        "the exemption must name the declaration that produced it, or a reader \
         cannot check it: {reason:?}"
    );
}

/// OFF: a neighbour in the same file is untouched.
///
/// If this fails, the fix is file-scoped and has reproduced the
/// `ScriptEntry` over-exemption on `pkg/cli.py` instead of on the manifest.
#[test]
fn a_neighbour_of_the_entry_function_stays_dead() {
    let reports = verdicts(&fixture(FLAT_MANIFEST));
    let stranded = verdict_for(&reports, "pkg/cli.py", "stranded")
        .unwrap_or_else(|| panic!("stranded vanished from the report: {reports:?}"));
    assert!(
        !stranded.2,
        "`stranded` shares a file with a console-script entry point and nothing \
         declares it; exempting it would be the file-scoped over-exemption this \
         change exists to avoid: {stranded:?}"
    );
}

/// OFF for the whole rule: with the declaration gone, the finding comes back.
///
/// Without this, an `analyze` that exempted every symbol would satisfy the
/// first test.
#[test]
fn without_the_declaration_the_entry_function_is_dead_again() {
    let reports = verdicts(&fixture("[project]\nname = \"fx\"\nversion = \"0.1.0\"\n"));
    let entry = verdict_for(&reports, "pkg/cli.py", "main_entry")
        .unwrap_or_else(|| panic!("main_entry is not in the report: {reports:?}"));
    assert!(
        !entry.2,
        "nothing declares `main_entry` in this manifest and nothing calls it, so \
         it is a real finding; an exemption that fires without the declaration \
         is not reading the declaration: {entry:?}"
    );
}

/// The `src/` layout, which is what this repository itself uses.
///
/// `pyproject.toml` here declares `dev = "devcouncil.cli.main:run_cli"` and the
/// module is at `src/devcouncil/cli/main.py`. A rule that only resolved the
/// flat layout would clear nothing in this repository.
#[test]
fn the_src_layout_resolves_too() {
    let extractions = vec![
        extract_file(
            "pyproject.toml",
            "[project]\nname = \"dc\"\n\n[project.scripts]\ndev = \"devcouncil.cli.main:run_cli\"\n",
        ),
        extract_file(
            "src/devcouncil/cli/main.py",
            "def run_cli():\n    return 1\n\n\ndef unused_helper():\n    return 2\n",
        ),
    ];
    let reports = verdicts(&extractions);
    let entry = verdict_for(&reports, "src/devcouncil/cli/main.py", "run_cli")
        .unwrap_or_else(|| panic!("run_cli is not in the report: {reports:?}"));
    assert!(entry.2, "the src/ layout must resolve as well: {entry:?}");
    let other = verdict_for(&reports, "src/devcouncil/cli/main.py", "unused_helper")
        .unwrap_or_else(|| panic!("unused_helper is not in the report: {reports:?}"));
    assert!(!other.2, "{other:?}");
}

/// A package `__init__.py` target resolves, and only the declared attribute.
#[test]
fn a_package_init_target_resolves_and_stops_at_the_declared_attribute() {
    let extractions = vec![
        extract_file(
            "pyproject.toml",
            "[project]\nname = \"fx\"\n\n[project.scripts]\nfxtool = \"pkg.cli:main\"\n",
        ),
        extract_file(
            "pkg/cli/__init__.py",
            "def main():\n    return 1\n\n\ndef sibling():\n    return 2\n",
        ),
    ];
    let reports = verdicts(&extractions);
    assert!(
        verdict_for(&reports, "pkg/cli/__init__.py", "main")
            .map(|v| v.2)
            .unwrap_or(false),
        "{reports:?}"
    );
    assert!(
        !verdict_for(&reports, "pkg/cli/__init__.py", "sibling")
            .map(|v| v.2)
            .unwrap_or(true),
        "only the attribute the manifest names is declared: {reports:?}"
    );
}

/// The manifest keeps its own file-scoped claim.
///
/// `ScriptEntry` on `pyproject.toml` is what makes the manifest an entry root
/// (`devmap-query/src/manifest.rs::is_entry_root`); the symbol annotation is an
/// addition to it, not a replacement.
#[test]
fn the_manifest_still_carries_its_script_entry() {
    let extractions = fixture(FLAT_MANIFEST);
    let manifest = extractions
        .iter()
        .find(|ext| ext.file_path == "pyproject.toml")
        .expect("the fixture holds a manifest");
    assert!(
        manifest
            .wiring
            .iter()
            .any(|w| w.kind == WiringKind::ScriptEntry && w.target_symbol == "pyproject.toml"),
        "{:?}",
        manifest.wiring
    );
    assert!(
        manifest
            .wiring
            .iter()
            .any(|w| w.kind == WiringKind::ConfigEntryPoint
                && w.target_symbol == "pkg/cli.py::main_entry"),
        "{:?}",
        manifest.wiring
    );
}

/// The cluster pass reaches the same verdict as the single-symbol pass.
///
/// `exempt_symbol_names` seeds `dead_clusters`. If the two disagree, the same
/// function is exempt in one answer and reported in the other — which is the
/// failure mode `exemption_seams.rs` exists to refuse for the other exemptions.
#[test]
fn the_exemption_reaches_the_cluster_pass_too() {
    let extractions = fixture(FLAT_MANIFEST);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let exempt = exempt_symbol_names(&extractions, &resolution);
    assert!(
        exempt.contains("pkg/cli.py::main_entry"),
        "the single-symbol pass clears the entry function and the cluster pass \
         does not: {exempt:?}"
    );
    assert!(
        !exempt.contains("pkg/cli.py::stranded"),
        "the cluster seed must not pick up the neighbour: {exempt:?}"
    );
}
