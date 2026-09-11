//! `devmap preview`: what an unsaved edit would do, computed without writing it.

use devmap_extract::*;
use devmap_query::*;
use devmap_resolve::*;
use devmap_store::Store;
use std::path::{Path, PathBuf};

/// A scratch directory named for the test using it.
///
/// The repository has no `tempfile` dev-dependency and this does not add one;
/// the convention here is a process-scoped path under the system temp dir. The
/// test name is part of it because these run in parallel in one process, so the
/// pid alone is not unique between them.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-preview-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Canonical, because the fixture records this directory as the
    // generation's `repo_root` and both production writers of that column do
    // the same. macOS `temp_dir()` is a symlink (`/var` -> `/private/var`), so
    // a non-canonical root would not prefix-match the paths built from it.
    dir.canonicalize().unwrap()
}

const LIB: &str = "\
def compute(rows, rate):
    total = 0
    for row in rows:
        if row.active:
            total += row.amount * rate
        else:
            total -= row.penalty
    return total


def helper(value):
    return value * 2
";

const CALLER: &str = "\
from lib import compute


def report(rows):
    return compute(rows, 1.5)
";

/// Build a store over a two-file fixture written into `dir`.
fn fixture(dir: &Path) -> Store {
    std::fs::write(dir.join("lib.py"), LIB).unwrap();
    std::fs::write(dir.join("caller.py"), CALLER).unwrap();
    let lib_path = dir.join("lib.py").to_string_lossy().into_owned();
    let caller_path = dir.join("caller.py").to_string_lossy().into_owned();

    let extractions = vec![
        extract_file(&lib_path, LIB),
        extract_file(&caller_path, CALLER),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    // `repo_root` is recorded because every production writer of a generation
    // records it — `devmap build` and the daemon's drain both store the
    // canonical root — and `preview` refuses a path it cannot show to be inside
    // that root. A fixture that left the column NULL was exercising a state no
    // real store is in.
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            devmap_store::GenerationWriteOpts {
                repo_root: Some(dir.to_string_lossy().into_owned()),
                ..devmap_store::GenerationWriteOpts::default()
            },
        )
        .unwrap();
    store
}

#[test]
fn removing_a_function_names_the_callers_it_would_break() {
    let dir = scratch("removing_a_function_names_the_callers_it_would_break");
    let store = fixture(dir.as_path());
    let lib_path = dir.as_path().join("lib.py").to_string_lossy().into_owned();

    // Drop `compute` entirely.
    let edited = "def helper(value):\n    return value * 2\n";
    let report = StoreQueryEngine::new(&store)
        .preview(&lib_path, edited, 2000, PREVIEW_CALLER_MIN_CONFIDENCE)
        .unwrap();

    assert!(report.delta_available);
    assert_eq!(report.parse_status, "clean");
    assert_eq!(report.compared_against, "disk");
    let removed: Vec<_> = report
        .symbols
        .iter()
        .filter(|s| s.change == PreviewChange::Removed)
        .map(|s| s.symbol_name.as_str())
        .collect();
    assert_eq!(removed, vec!["compute"]);

    // The caller must be found. This is the assertion that catches matching on
    // bare names: `generation_edges.target_symbol` holds `path::Name`, and a
    // bare-name lookup returns nothing while looking exactly like "no callers".
    assert!(
        report
            .broken_callers
            .items
            .iter()
            .any(|c| c.caller_symbol.contains("report")),
        "the call from caller.py was not reported: {:?}",
        report.broken_callers.items
    );
}

/// The failure this feature must not have.
#[test]
fn a_buffer_that_does_not_parse_reports_no_delta_at_all() {
    let dir = scratch("a_buffer_that_does_not_parse_reports_no_delta_at_all");
    let store = fixture(dir.as_path());
    let lib_path = dir.as_path().join("lib.py").to_string_lossy().into_owned();

    // Rust source at a .py path: the Python grammar cannot make sense of it.
    let garbage = "fn main() -> Result<(), Box<dyn Error>> { let x = vec![1,2,3]; Ok(()) }\n";
    let report = StoreQueryEngine::new(&store)
        .preview(&lib_path, garbage, 2000, PREVIEW_CALLER_MIN_CONFIDENCE)
        .unwrap();

    if report.parse_status == "failed" {
        assert!(
            !report.delta_available,
            "a failed parse must withhold the delta"
        );
        assert!(report.symbols.is_empty());
        assert_eq!(report.broken_callers.total, 0);
        assert!(report.degraded_reason.is_some());
    } else {
        // Grammars recover aggressively, so this may parse as `partial`. The
        // requirement then is that the reader is warned, not that nothing is
        // reported — a partial parse can hide a symbol and make it look removed.
        assert_eq!(report.parse_status, "partial");
        assert!(
            report.degraded_reason.is_some(),
            "a degraded parse reported a delta with no warning attached"
        );
    }
}

#[test]
fn a_body_change_is_not_reported_as_breaking_callers() {
    let dir = scratch("a_body_change_is_not_reported_as_breaking_callers");
    let store = fixture(dir.as_path());
    let lib_path = dir.as_path().join("lib.py").to_string_lossy().into_owned();

    // Same declaration, different body.
    let edited = LIB.replace("total -= row.penalty", "total -= row.penalty * 2");
    assert_ne!(edited, LIB, "fixture edit did not apply");
    let report = StoreQueryEngine::new(&store)
        .preview(&lib_path, &edited, 2000, PREVIEW_CALLER_MIN_CONFIDENCE)
        .unwrap();

    assert!(report
        .symbols
        .iter()
        .any(|s| s.symbol_name == "compute" && s.change == PreviewChange::BodyChanged));
    assert_eq!(
        report.broken_callers.total, 0,
        "a body change does not break a call site and must not be listed as doing so"
    );
}

#[test]
fn a_declaration_change_outranks_the_body_change_it_implies() {
    let dir = scratch("a_declaration_change_outranks_the_body_change_it_implies");
    let store = fixture(dir.as_path());
    let lib_path = dir.as_path().join("lib.py").to_string_lossy().into_owned();

    let edited = LIB.replace(
        "def compute(rows, rate):",
        "def compute(rows, rate, scale):",
    );
    assert_ne!(edited, LIB, "fixture edit did not apply");
    let report = StoreQueryEngine::new(&store)
        .preview(&lib_path, &edited, 2000, PREVIEW_CALLER_MIN_CONFIDENCE)
        .unwrap();

    let compute = report
        .symbols
        .iter()
        .find(|s| s.symbol_name == "compute")
        .expect("compute is in the report");
    assert_eq!(
        compute.change,
        PreviewChange::SignatureChanged,
        "adding a parameter is a declaration change, not a body change"
    );
    assert!(
        report.broken_callers.total > 0,
        "a signature change with a live caller reported nothing affected"
    );
}

#[test]
fn an_unchanged_buffer_reports_nothing() {
    let dir = scratch("an_unchanged_buffer_reports_nothing");
    let store = fixture(dir.as_path());
    let lib_path = dir.as_path().join("lib.py").to_string_lossy().into_owned();

    let report = StoreQueryEngine::new(&store)
        .preview(&lib_path, LIB, 2000, PREVIEW_CALLER_MIN_CONFIDENCE)
        .unwrap();
    assert!(
        report.symbols.is_empty(),
        "an identical buffer produced a delta: {:?}",
        report.symbols
    );
    assert_eq!(report.broken_callers.total, 0);
}

#[test]
fn preview_writes_nothing() {
    let dir = scratch("preview_writes_nothing");
    let store = fixture(dir.as_path());
    let lib_path = dir.as_path().join("lib.py").to_string_lossy().into_owned();
    let before = std::fs::read_to_string(&lib_path).unwrap();
    let generation_before = store.latest_generation_id().unwrap();

    StoreQueryEngine::new(&store)
        .preview(
            &lib_path,
            "def nothing():\n    pass\n",
            2000,
            PREVIEW_CALLER_MIN_CONFIDENCE,
        )
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(&lib_path).unwrap(),
        before,
        "preview modified the file it was asked to speculate about"
    );
    assert_eq!(
        store.latest_generation_id().unwrap(),
        generation_before,
        "preview committed a generation"
    );
}

/// The `devmap preview` command refuses a path outside the repository too.
///
/// The containment rule lives in the engine, which both callers go through, so
/// this is here to pin that the CLI really is one of them — a second entry point
/// that skipped the check would look exactly like this test not existing.
#[test]
fn the_cli_refuses_to_preview_a_path_outside_the_repository() {
    use std::process::Command;

    let dir = scratch("cli_refuses_escape");
    let content = dir.join("candidate.py");
    std::fs::write(&content, "").unwrap();
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("lib.py"), LIB).unwrap();
    std::fs::write(
        dir.join("secrets.py"),
        "def api_key():\n    return 'hunter2'\n",
    )
    .unwrap();
    let db = devmap_extract::paths::store_path(&repo);

    let built = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--db")
        .arg(&db)
        .arg("build")
        .arg(&repo)
        .output()
        .expect("devmap build must run");
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let escaped = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--db")
        .arg(&db)
        .arg("preview")
        .arg("--file")
        .arg("../secrets.py")
        .arg("--content")
        .arg(&content)
        .output()
        .expect("devmap preview must run");
    assert!(
        !escaped.status.success(),
        "the CLI previewed a file outside the repository: {}",
        String::from_utf8_lossy(&escaped.stdout)
    );
    let complaint = String::from_utf8_lossy(&escaped.stderr);
    assert!(
        complaint.contains("traversal") || complaint.contains("outside"),
        "the refusal must name the containment rule: {complaint}"
    );
    assert!(
        !String::from_utf8_lossy(&escaped.stdout).contains("api_key"),
        "the refused preview still leaked the target file's symbols"
    );

    // The same command against a file inside the repository still works, so the
    // guard is containment rather than a blanket refusal.
    let allowed = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--db")
        .arg(&db)
        .arg("preview")
        .arg("--file")
        .arg("lib.py")
        .arg("--content")
        .arg(&content)
        .output()
        .expect("devmap preview must run");
    assert!(
        allowed.status.success(),
        "an in-repository preview was refused: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ra4_preview_refuses_an_oversized_saved_source() {
    let dir = scratch("ra4_saved_size");
    let store = fixture(&dir);
    let path = dir.join("lib.py");
    std::fs::write(
        &path,
        format!("{LIB}\n#{}", "x".repeat(MAX_SOURCE_BYTES as usize)),
    )
    .unwrap();
    let report = StoreQueryEngine::new(&store)
        .preview(
            &path.to_string_lossy(),
            LIB,
            2000,
            PREVIEW_CALLER_MIN_CONFIDENCE,
        )
        .unwrap();
    assert!(
        !report.delta_available,
        "an unreadable baseline cannot support a delta: {report:?}"
    );
    assert!(report.degraded_reason.is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ra4_preview_refuses_an_oversized_candidate_before_parsing() {
    let dir = scratch("ra4_candidate_size");
    let store = fixture(&dir);
    let source = format!("{LIB}\n#{}", "x".repeat(MAX_SOURCE_BYTES as usize));
    let report = StoreQueryEngine::new(&store).preview(
        &dir.join("lib.py").to_string_lossy(),
        &source,
        2000,
        PREVIEW_CALLER_MIN_CONFIDENCE,
    );
    assert!(
        report.is_err(),
        "an oversized candidate must be rejected at the input boundary"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
