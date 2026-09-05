//! A cold build must not read a file that lives outside the repository.
//!
//! The rule had three owners and only one of them enforced it. `preview`
//! refuses a `--file` that "resolves outside the indexed repository root"; the
//! drain refused *every* symlink, in-repository ones included; and the cold
//! walk in [`devmap_extract::collect_sources_with_report`] enforced nothing at
//! all — `WalkBuilder` is configured not to *descend* through symlinks, but
//! `Path::is_file` and `fs::read_to_string` both follow one, so a single
//! symlinked file was read through without anyone asking where it pointed.
//! There is now one owner, [`devmap_extract::candidate_kind`], and the drain
//! asks it too.
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

/// A link that will not resolve is *unknown*, and unknown must be recorded.
///
/// `Path::is_file` follows the link and collapses ENOENT-on-the-target into the
/// same `false` it returns for a directory, so the walk stepped over a dangling
/// `src/gone.py` in complete silence: it was not indexed, not refused, and not
/// mentioned in the report at all. `escapes_root` had the right answer the
/// whole time ("target could not be resolved") and was asked three lines too
/// late to be heard.
#[test]
fn a_dangling_symlink_is_recorded_as_a_refusal_rather_than_passed_over_in_silence() {
    let root = scratch("dangling");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    std::os::unix::fs::symlink(root.join("nowhere.py"), root.join("src/gone.py")).unwrap();

    let (sources, report) = collect_sources_with_report(&root).expect("discovery runs");
    assert!(
        !sources.iter().any(|(path, _)| path == "src/gone.py"),
        "a link with no target has no bytes to index"
    );
    assert!(
        skip_reason(&report, "src/gone.py").is_some_and(DiscoverySkipReason::is_refusal),
        "a link whose target will not resolve is coverage loss, not silence: {:?}",
        report.skipped_paths
    );
}

/// The same rule for a link that resolves to itself: `canonicalize` fails with
/// ELOOP, which is "could not be established", not "inside".
#[test]
fn a_symlink_loop_is_recorded_as_a_refusal() {
    let root = scratch("loop");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    std::os::unix::fs::symlink("loop.py", root.join("src/loop.py")).unwrap();

    let (sources, report) = collect_sources_with_report(&root).expect("discovery runs");
    assert!(
        !sources.iter().any(|(path, _)| path == "src/loop.py"),
        "a self-referential link names no bytes"
    );
    assert!(
        skip_reason(&report, "src/loop.py").is_some_and(DiscoverySkipReason::is_refusal),
        "ELOOP is a refusal, not a silent pass-over: {:?}",
        report.skipped_paths
    );
}

/// The size ceiling has to be measured on the bytes that will be *read*.
///
/// The walk sized a candidate with `ignore::DirEntry::metadata()`, which does
/// not follow the link — it reports the size of the link itself, a path length
/// of a few dozen bytes. `fs::read_to_string` three lines below follows it. So
/// a file over `MAX_SOURCE_BYTES`, reached through an in-repository symlink,
/// walked straight through the ceiling that exists to bound this loop's memory.
#[test]
fn a_file_over_the_ceiling_reached_through_an_in_root_symlink_is_still_refused_by_size() {
    let root = scratch("oversize-link");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("vendor")).unwrap();
    let big = root.join("vendor/big.py");
    fs::write(&big, "x = 1\n".repeat(400_000)).unwrap();
    assert!(
        fs::metadata(&big).unwrap().len() > devmap_extract::MAX_SOURCE_BYTES,
        "the fixture must be over the ceiling"
    );
    std::os::unix::fs::symlink(&big, root.join("src/big.py")).unwrap();

    let (sources, report) = collect_sources_with_report(&root).expect("discovery runs");
    assert!(
        !sources.iter().any(|(path, _)| path == "src/big.py"),
        "the ceiling must bound the bytes actually read, not the link's own size"
    );
    assert!(
        matches!(
            skip_reason(&report, "src/big.py"),
            Some(DiscoverySkipReason::Oversized { .. })
        ),
        "the refusal must name the size, as it does for the real path: {:?}",
        report.skipped_paths
    );
}

/// A link to a directory inside the repository is not a way in: the walk does
/// not descend it, and the files under the target are reached under their real
/// names. Pinned so the drain, which now asks the same owner, can be held to
/// the same answer.
#[test]
fn a_symlink_to_a_directory_inside_the_repository_is_not_descended() {
    let root = scratch("dirlink");
    fs::create_dir_all(root.join("shared/pkg")).unwrap();
    fs::write(root.join("shared/pkg/mod.py"), "def m():\n    return 1\n").unwrap();
    std::os::unix::fs::symlink(root.join("shared/pkg"), root.join("pkg")).unwrap();

    let (sources, _) = collect_sources_with_report(&root).expect("discovery runs");
    let paths: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();
    assert!(
        paths.contains(&"shared/pkg/mod.py"),
        "the real path must be indexed: {paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.starts_with("pkg/")),
        "the walk must not descend a link, or the same bytes are indexed twice \
         under two names: {paths:?}"
    );
}
