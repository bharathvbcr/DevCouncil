//! A cold build must not read a file that lives outside the repository.
//!
//! The rule has three owners and only two of them enforced it. `preview`
//! refuses a `--file` that "resolves outside the indexed repository root", and
//! `classify_pending_entry` refuses a queued path that is not "a regular file
//! or directory", which is how the drain declines a symlink. The cold walk in
//! [`devmap_extract::collect_sources_with_report`] did neither: `WalkBuilder`
//! is configured not to *descend* through symlinks, but `Path::is_file` and
//! `fs::read_to_string` both follow one, so a single symlinked file was read
//! through without anyone asking where it pointed.
//!
//! Measured with the release binary on a two-file repository containing
//! `src/a.py` and `src/creds.py -> <outside>/credentials.py`:
//!
//! ```text
//! $ devmap build <root>
//!   {"files_indexed": 2, "symbols": 4, "edges": 2, "discovery_refused_files": 0}
//! $ sqlite3 devmap.sqlite 'select path from paths'
//!   src/a.py
//!   src/creds.py
//! $ devmap preview --file src/creds.py
//!   Error: "src/creds.py" resolves outside the indexed repository root
//! ```
//!
//! The build indexed symbols the query layer then refused to show — the two
//! halves of one tool disagreeing about the boundary of the repository, with
//! the permissive half being the one that reads the bytes.

use std::fs;
use std::path::{Path, PathBuf};

use devmap_extract::collect_sources_with_report;
use devmap_extract::model::DiscoverySkipReason;

/// A canonical scratch root. macOS `temp_dir()` is itself a symlink
/// (`/var` -> `/private/var`), so a non-canonical root would make every
/// containment comparison in this file vacuous.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-discovery-esc-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn skip_reason<'a>(
    report: &'a devmap_extract::model::DiscoveryReport,
    path: &str,
) -> Option<&'a DiscoverySkipReason> {
    report
        .skipped_paths
        .iter()
        .find(|(candidate, _)| candidate == path)
        .map(|(_, reason)| reason)
}

#[test]
fn a_symlink_pointing_out_of_the_repository_is_not_read() {
    let base = scratch("outside");
    let root = base.join("repo");
    let outside = base.join("elsewhere");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&outside).unwrap();

    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(
        outside.join("credentials.py"),
        "SECRET = \"AKIAIOSFODNN7EXAMPLE\"\n\n\ndef leak():\n    return SECRET\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.join("credentials.py"), root.join("src/creds.py")).unwrap();

    let (sources, report) = collect_sources_with_report(&root).expect("discovery runs");

    let paths: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["src/a.py"],
        "discovery read a file that lives outside the repository it was pointed at"
    );
    assert!(
        !sources.iter().any(|(_, body)| body.contains("AKIAIOSF")),
        "the contents of a file outside the root reached the extractor"
    );

    // Class A: refusing it is only half the rule. A file the indexer was
    // pointed at and would not read is a hole, and the report has to carry it
    // or the graph looks complete.
    let reason = skip_reason(&report, "src/creds.py")
        .expect("the refusal must be recorded, not silently dropped");
    assert!(
        reason.is_refusal(),
        "a path the build declined to read is coverage loss, got {reason:?}"
    );
}

/// A symlink to a file inside the repository is ordinary — a monorepo's shared
/// config, a vendored header — and the bytes it names are in the tree either
/// way. Refusing it would cost coverage for no containment gain, and it is the
/// case that makes the assertion above mean "outside" rather than "symlink".
#[test]
fn a_symlink_pointing_inside_the_repository_is_still_read() {
    let root = scratch("inside");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();
    fs::write(root.join("shared/util.py"), "def util():\n    return 2\n").unwrap();
    std::os::unix::fs::symlink(root.join("shared/util.py"), root.join("src/util.py")).unwrap();

    let (sources, _) = collect_sources_with_report(&root).expect("discovery runs");
    let paths: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();
    assert!(
        paths.contains(&"src/util.py") && paths.contains(&"shared/util.py"),
        "an in-repository symlink must still be indexed, got {paths:?}"
    );
}

/// `..` inside the link target is the same escape spelled differently, and a
/// prefix test on the uncanonicalised path waves it through.
#[test]
fn a_relative_symlink_that_climbs_out_of_the_repository_is_not_read() {
    let base = scratch("relative");
    let root = base.join("repo");
    let outside = base.join("elsewhere");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.py"), "TOKEN = \"hunter2\"\n").unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    std::os::unix::fs::symlink(
        Path::new("../../elsewhere/secret.py"),
        root.join("src/secret.py"),
    )
    .unwrap();

    let (sources, report) = collect_sources_with_report(&root).expect("discovery runs");
    assert!(
        !sources.iter().any(|(path, _)| path == "src/secret.py"),
        "a `..` in the link target climbs out of the repository just as an \
         absolute target does"
    );
    assert!(
        !sources.iter().any(|(_, body)| body.contains("hunter2")),
        "the contents of a file outside the root reached the extractor"
    );
    assert!(
        skip_reason(&report, "src/secret.py").is_some_and(DiscoverySkipReason::is_refusal),
        "the refusal must be recorded as coverage loss: {:?}",
        report.skipped_paths
    );
}
