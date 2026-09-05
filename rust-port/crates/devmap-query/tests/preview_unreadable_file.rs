//! `preview` must not report a clean delta against a file it could not read.
//!
//! The whole point of `preview` is "which callers would this edit break, before
//! I write it". It answers by diffing the buffer against the file on disk.
//!
//! `std::fs::read_to_string(..).ok()` collapsed two different facts into one
//! `None`: *there is no such file* and *the file is there and I could not read
//! it*. `None` set `compared_against: "nothing"`, documented as **no such file,
//! so every symbol is an addition** — so a symbol the edit genuinely REMOVES is
//! reported as an addition of everything else, `degraded_reason` stays null and
//! `delta_available` stays true.
//!
//! The caller asked "what would break" and was handed a clean bill of health by
//! a comparison that never happened. That is this repository's Class A rule at
//! the surface where it costs the most.

use devmap_extract::extract_file;
use devmap_query::StoreQueryEngine;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::{Path, PathBuf};

/// A scratch directory named for the test using it. No `tempfile`
/// dev-dependency exists in this workspace and this does not add one.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-preview-unreadable-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`) and the
    // recorded repo root is always canonical, so the fixture's must be too.
    dir.canonicalize().unwrap()
}

const MODULE: &str = "def alpha():\n    return 1\n\n\ndef beta():\n    return 2\n";

fn fixture(root: &Path) -> Store {
    let extractions = vec![extract_file("mod.py", MODULE)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    store
}

/// The buffer drops `beta`. Against readable content that is a removal.
const EDIT_REMOVING_BETA: &str = "def alpha():\n    return 1\n";

#[test]
fn a_readable_file_reports_the_removal() {
    // The control. Without it, a fix that reported every file as unreadable
    // would satisfy the test below.
    let root = scratch("readable");
    std::fs::write(root.join("mod.py"), MODULE).unwrap();
    let store = fixture(&root);

    let report = StoreQueryEngine::new(&store)
        .preview("mod.py", EDIT_REMOVING_BETA, 10_000, 0.0)
        .expect("preview succeeds");

    assert_eq!(report.compared_against, "disk");
    assert!(report.delta_available);
    let symbols: Vec<String> = report.symbols.iter().map(|s| format!("{s:?}")).collect();
    assert!(
        symbols.iter().any(|s| s.contains("beta")),
        "the removal of `beta` must be reported, got {symbols:?}"
    );
}

#[test]
fn an_unreadable_file_is_not_reported_as_a_clean_delta() {
    let root = scratch("unreadable");
    // Invalid UTF-8: the file exists and `read_to_string` fails. This is the
    // audit's own reproduction, and it needs no permission changes, so it
    // behaves identically when the suite runs as root in CI.
    std::fs::write(root.join("mod.py"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
    let store = fixture(&root);

    let report = StoreQueryEngine::new(&store)
        .preview("mod.py", EDIT_REMOVING_BETA, 10_000, 0.0)
        .expect("preview must answer rather than fail");

    assert_ne!(
        report.compared_against, "nothing",
        "`nothing` means there was genuinely no prior content, which licenses \
         'every symbol is an addition'. Reusing it for an unreadable file makes a \
         real removal disappear."
    );
    assert_eq!(report.compared_against, "unreadable");
    assert!(
        !report.delta_available,
        "no previous content means no comparison happened, so there is no delta to \
         offer; reporting `delta_available: true` hands the caller a clean bill of \
         health from a check that did not run"
    );
    let reason = report.degraded_reason.unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "the caller must be told why, or it cannot tell this from a genuine 'nothing breaks'"
    );
}

#[test]
fn a_genuinely_absent_file_still_reports_nothing() {
    // The third state must stay distinct from the other two: a file that is not
    // there really does make every symbol an addition, and collapsing this into
    // the `unreadable` case would be the same defect facing the other way.
    let root = scratch("absent");
    let store = fixture(&root);

    let report = StoreQueryEngine::new(&store)
        .preview("mod.py", EDIT_REMOVING_BETA, 10_000, 0.0)
        .expect("preview succeeds");

    assert_eq!(report.compared_against, "nothing");
    assert!(
        report.delta_available,
        "an absent file is a complete comparison against nothing, not a failed one"
    );
}
