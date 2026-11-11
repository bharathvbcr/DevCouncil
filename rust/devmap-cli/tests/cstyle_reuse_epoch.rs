//! `RESOLUTION_REUSE_EPOCH` is what keeps a store built before the CStyle split
//! from re-serving its cross-group edges.
//!
//! Per-file resolution blobs are keyed by content hash alone, so an unchanged
//! Java file would keep a pre-split `Cls.h -> c/lib.c::only_c` edge forever
//! unless the epoch moves the blobs to a directory the new build never reads.
//!
//! A "warm store built by the base resolver" is staged here rather than built:
//! a test cannot run the old resolver, so the store is built by this tree and
//! the one edge the old resolver would have written is added to the two places
//! it would have been (the committed generation and the file's reuse blob),
//! under the directory name the pre-split build used, `resolution-reuse-1`.
//! With the constant at "1" that directory is the live one and the edge
//! survives; at "2" it is never read.

use std::process::Command;
use std::sync::Arc;

use devmap_extract::model::EdgeKind;
use devmap_resolve::{LangFamily, Resolution, ResolvedEdge};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn build(root: &std::path::Path, extra: &[&str]) -> String {
    let mut args = vec!["build", "."];
    args.extend_from_slice(extra);
    let out = Command::new(devmap())
        .args(&args)
        .current_dir(root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn scratch(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-cstyle-epoch-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("c")).unwrap();
    std::fs::create_dir_all(root.join("java")).unwrap();
    std::fs::create_dir_all(root.join("util")).unwrap();
    std::fs::write(root.join("c/lib.c"), "int only_c(int x) { return x; }\n").unwrap();
    std::fs::write(
        root.join("java/Cls.java"),
        "class Cls { void h() { only_c(1); } }\n",
    )
    .unwrap();
    std::fs::write(root.join("util/other.py"), "def f():\n    return 1\n").unwrap();
    root
}

fn db_path(root: &std::path::Path) -> std::path::PathBuf {
    devmap_extract::paths::store_path(root)
}

const STALE: &str = "java/Cls.java::Cls.h>c/lib.c::only_c:Calls:0.90000";

fn has_stale_edge(root: &std::path::Path) -> bool {
    let store = devmap_store::Store::open(db_path(root)).unwrap();
    store
        .latest_edges_for_test()
        .unwrap()
        .iter()
        .any(|row| row == STALE)
}

/// A store the pre-split resolver would have left behind: the stale cross-group
/// edge is in the latest generation and in `Cls.java`'s reuse blob under
/// `resolution-reuse-1`.
fn warm_store_from_the_base_resolver(label: &str) -> std::path::PathBuf {
    stage_store(label, false)
}

/// Stamp every stored payload with the extractor identity ONE BELOW this
/// binary's, which is exactly the "v99 over a v98 store" case.
///
/// The field and comparison are the no-change shortcut's own: `build.rs:588`
/// gates the early return on `store.latest_generation_payload_is_current()`,
/// which (`devmap-store/src/db/generation.rs:582-613`) compares each latest
/// payload's `file_payloads.analyzer_version` with
/// `devmap_extract::cache::current_payload_identity(language).1`, that is
/// `"{ANALYZER_VERSION}:extract-v{EXTRACTION_SCHEMA_VERSION}"`
/// (`devmap-extract/src/cache.rs:875`). The value written here is that string
/// with the schema number lowered by one, per language.
fn stamp_older_extractor(root: &std::path::Path) {
    let conn = rusqlite::Connection::open(db_path(root)).unwrap();
    let languages: Vec<String> = conn
        .prepare("SELECT DISTINCT language FROM file_payloads")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!languages.is_empty(), "the store carries payloads");
    for language in languages {
        let (_, current) = devmap_extract::cache::current_payload_identity(&language);
        let (prefix, n) = current
            .rsplit_once("extract-v")
            .expect("an extract-vNN identity");
        let older = format!("{prefix}extract-v{}", n.parse::<u32>().unwrap() - 1);
        let stored: String = conn
            .query_row(
                "SELECT DISTINCT analyzer_version FROM file_payloads WHERE language = ?1",
                [&language],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, current,
            "{language}: the store was built by this binary"
        );
        conn.execute(
            "UPDATE file_payloads SET analyzer_version = ?1 WHERE language = ?2",
            [&older, &language],
        )
        .unwrap();
    }
}

/// Overwrite (or, with `None`, remove) the resolver identity the store's own
/// write path stamped into the latest generation's `analysis_json`. This is the
/// store as an older binary left it: `Some("1")` is a generation resolved under
/// epoch 1, `None` one written before the stamp existed.
fn stamp_resolver(root: &std::path::Path, epoch: Option<&str>) {
    let conn = rusqlite::Connection::open(db_path(root)).unwrap();
    let changed = match epoch {
        Some(epoch) => conn.execute(
            "UPDATE generations SET analysis_json = json_set(analysis_json, '$.resolver_epoch', ?1)
              WHERE id = (SELECT max(id) FROM generations)",
            [epoch],
        ),
        None => conn.execute(
            "UPDATE generations SET analysis_json = json_remove(analysis_json, '$.resolver_epoch')
              WHERE id = (SELECT max(id) FROM generations)",
            [],
        ),
    }
    .unwrap();
    assert_eq!(changed, 1);
}

fn status_json(root: &std::path::Path, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["--json", "status"];
    args.extend_from_slice(extra);
    let out = Command::new(devmap())
        .args(&args)
        .current_dir(root)
        .output()
        .expect("devmap status");
    assert!(
        out.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("status prints one JSON document")
}

fn stage_store(label: &str, older_extractor: bool) -> std::path::PathBuf {
    let root = scratch(label);
    build(&root, &[]);
    assert!(
        !has_stale_edge(&root),
        "this tree must not itself produce a Java -> C edge"
    );

    // The blob.
    let state = devmap_extract::paths::state_dir(&root);
    let legacy = state.join("resolution-reuse-1");
    let current = state.join(format!(
        "resolution-reuse-{}",
        devmap_resolve::resolver::RESOLUTION_REUSE_EPOCH
    ));
    if current != legacy {
        std::fs::rename(&current, &legacy).unwrap();
    }
    let stale = serde_json::to_value(ResolvedEdge::resolved(
        "java/Cls.java".into(),
        "c/lib.c".into(),
        "java/Cls.java::Cls.h".into(),
        "c/lib.c::only_c".into(),
        EdgeKind::Calls,
        Arc::new(Resolution::UniqueGlobal {
            target_symbol: "c/lib.c::only_c".into(),
            target_file: "c/lib.c".into(),
            family: LangFamily::from_lang("c"),
        }),
        None,
    ))
    .unwrap();
    use sha2::{Digest, Sha256};
    let blob = legacy.join(format!("{:x}.json", Sha256::digest(b"java/Cls.java")));
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&blob).unwrap()).unwrap();
    value["edges"].as_array_mut().unwrap().push(stale);
    std::fs::write(&blob, serde_json::to_vec(&value).unwrap()).unwrap();

    // The committed generation. `generation_edges` is a view over the
    // time-versioned `edge_rows`; an open row is one with no `valid_to`.
    let conn = rusqlite::Connection::open(db_path(&root)).unwrap();
    let added = conn
        .execute(
            "INSERT INTO edge_rows
               (source_file_id, target_file_id, source_symbol, target_symbol,
                edge_kind, confidence, resolution, candidate_total, valid_from, valid_to)
             SELECT (SELECT id FROM paths WHERE path = 'java/Cls.java'),
                    (SELECT id FROM paths WHERE path = 'c/lib.c'),
                    'java/Cls.java::Cls.h', 'c/lib.c::only_c', 'Calls', 0.9,
                    'UniqueGlobal', NULL, max(id), NULL
             FROM generations",
            [],
        )
        .unwrap();
    assert_eq!(added, 1, "the stale edge was staged");
    drop(conn);
    assert!(
        has_stale_edge(&root),
        "the staged store carries the stale edge"
    );
    if older_extractor {
        stamp_older_extractor(&root);
    }
    root
}

/// The resolving build removes every `resolution-reuse-<other epoch>` directory
/// (`remove_old_reuse_epochs`), so the pre-split `resolution-reuse-1` is gone.
fn assert_legacy_reuse_dir_removed(root: &std::path::Path) {
    let legacy = devmap_extract::paths::state_dir(root).join("resolution-reuse-1");
    assert!(
        devmap_resolve::resolver::RESOLUTION_REUSE_EPOCH == "1" || !legacy.exists(),
        "{} was not removed by the resolving build",
        legacy.display()
    );
}

fn edit_unrelated_file(root: &std::path::Path) {
    std::fs::write(root.join("util/other.py"), "def f():\n    return 2\n").unwrap();
}

#[test]
fn an_unrelated_edit_then_a_normal_build_drops_the_stale_cross_group_edge() {
    let root = warm_store_from_the_base_resolver("edit");
    edit_unrelated_file(&root);
    let out = build(&root, &[]);
    assert!(
        !has_stale_edge(&root),
        "a pre-split cross-group edge survived an unrelated edit; build said:\n{out}"
    );
    assert_legacy_reuse_dir_removed(&root);
}

#[test]
fn an_affected_build_drops_the_stale_cross_group_edge() {
    let root = warm_store_from_the_base_resolver("affected");
    edit_unrelated_file(&root);
    let out = build(&root, &["--affected", "util/other.py"]);
    assert!(
        !has_stale_edge(&root),
        "a pre-split cross-group edge survived --affected; build said:\n{out}"
    );
    assert_legacy_reuse_dir_removed(&root);
}

/// `--full` never reads reuse blobs, so it drops the edge at either epoch. This
/// is a guard that the full path keeps doing so, not evidence about the bump.
#[test]
fn a_full_build_drops_the_stale_cross_group_edge() {
    let root = warm_store_from_the_base_resolver("full");
    let out = build(&root, &["--full"]);
    assert!(
        !has_stale_edge(&root),
        "a pre-split cross-group edge survived --full; build said:\n{out}"
    );
}

/// The forcing step the no-change shortcut does have: a store stamped by an
/// older extractor is not "still current", so a ZERO-EDIT build re-extracts and
/// re-resolves instead of returning early. Combined with the epoch (the reuse
/// blobs of a pre-split store sit under `resolution-reuse-1`, which this build
/// never reads), the pre-split cross-group edge is gone. This is what an
/// extract-schema bump does to every store built before it.
#[test]
fn a_zero_edit_build_over_an_older_extractor_store_drops_the_stale_edge() {
    let root = stage_store("zero-older", true);
    let out = build(&root, &[]);
    assert!(
        !out.contains("No source changes"),
        "an older extractor identity must defeat the no-change shortcut; build said:\n{out}"
    );
    assert!(
        !has_stale_edge(&root),
        "a pre-split cross-group edge survived a zero-edit build over an older extractor; build said:\n{out}"
    );
    assert_legacy_reuse_dir_removed(&root);
}

/// The paired control that makes the older-extractor and older-resolver tests
/// discriminate: the SAME staged store and the same zero-edit build, with the
/// extractor stamp AND the resolver stamp equal to this binary's, takes the
/// shortcut and keeps the stale edge. `stage_store` builds with this binary, so
/// both stamps are current, and the staged edge is a forgery under a matching
/// stamp: the shortcut trusts the stamp, by design. Only the stamps differ
/// between this test and the ones that re-resolve.
#[test]
fn a_zero_edit_build_over_a_current_resolver_store_trusts_the_stamp() {
    let root = stage_store("zero-current", false);
    let out = build(&root, &[]);
    assert!(
        out.contains("No source changes"),
        "expected the shortcut; build said:\n{out}"
    );
    assert!(has_stale_edge(&root), "the stale edge must still be there");
    assert!(
        devmap_extract::paths::state_dir(&root)
            .join("resolution-reuse-1")
            .exists()
            || devmap_resolve::resolver::RESOLUTION_REUSE_EPOCH == "1",
        "the shortcut returns before the reuse directories are pruned"
    );
}

/// The zero-edit build over a store resolved under an OLDER resolver epoch. The
/// extractor identity is current (the staged store was built by this binary), so
/// only the resolver stamp can defeat the no-change shortcut. This replaces the
/// ignored pin `a_zero_edit_build_keeps_the_stale_edge_by_the_no_change_shortcut`,
/// which recorded the defect (ft-c1d8cb8e) instead of failing on it.
#[test]
fn a_zero_edit_build_over_an_older_resolver_store_drops_the_stale_edge() {
    let root = stage_store("zero-older-resolver", false);
    stamp_resolver(&root, Some("1"));
    let out = build(&root, &[]);
    assert!(
        !out.contains("No source changes"),
        "an older resolver epoch must defeat the no-change shortcut; build said:\n{out}"
    );
    assert!(
        !has_stale_edge(&root),
        "a base-era cross-group edge survived a zero-edit build over an older resolver; build said:\n{out}"
    );
    assert_legacy_reuse_dir_removed(&root);
    let again = build(&root, &[]);
    assert!(
        again.contains("No source changes"),
        "the re-resolve stamps this binary's epoch, so the next zero-edit build is the shortcut; build said:\n{again}"
    );
}

/// A generation written before the stamp existed has no resolver identity, and
/// an unknown identity is not a matching one. It re-resolves once (no re-parse),
/// and the second zero-edit build is the shortcut again.
#[test]
fn a_zero_edit_build_over_an_unstamped_store_re_resolves_once() {
    let root = stage_store("zero-unstamped", false);
    stamp_resolver(&root, None);
    let out = build(&root, &[]);
    assert!(
        !out.contains("No source changes"),
        "an unstamped generation must defeat the no-change shortcut; build said:\n{out}"
    );
    assert!(
        !has_stale_edge(&root),
        "a stale edge survived a zero-edit build over an unstamped store; build said:\n{out}"
    );
    let again = build(&root, &[]);
    assert!(
        again.contains("No source changes"),
        "the unstamped store re-resolves ONCE; the second build said:\n{again}"
    );
}

/// The `--affected` row of the card's table: no edit, `--affected <file>`.
#[test]
fn an_affected_build_with_no_edit_over_an_older_resolver_store_drops_the_stale_edge() {
    let root = stage_store("affected-older-resolver", false);
    stamp_resolver(&root, Some("1"));
    let out = build(&root, &["--affected", "util/other.py"]);
    assert!(
        !out.contains("No source changes"),
        "an older resolver epoch must defeat the no-change shortcut under --affected; build said:\n{out}"
    );
    assert!(
        !has_stale_edge(&root),
        "a base-era cross-group edge survived --affected with no edit over an older resolver; build said:\n{out}"
    );
}

/// `devmap status` names a resolver mismatch with its own reason, reports the
/// analyzer as not fresh, and does NOT ask SessionStart for an auto-rebuild: a
/// re-resolve can outlast the hook budget, so it lands on the next detached or
/// manual build. The older-extractor store beside it is the contrast, which does
/// ask for one.
#[test]
fn status_names_a_resolver_mismatch_and_does_not_auto_rebuild_on_it() {
    let root = stage_store("status-older-resolver", false);
    stamp_resolver(&root, Some("1"));
    let status = status_json(&root, &["--auto-rebuild"]);
    let reason = status["degraded_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("resolver epoch 1") && reason.contains("devmap build"),
        "the resolver mismatch needs its own reason; status said: {status}"
    );
    assert!(!reason.contains("obsolete"), "{reason}");
    assert_eq!(
        status["analyzer_freshness"],
        serde_json::json!(false),
        "{status}"
    );
    assert_eq!(status["is_fresh"], serde_json::json!(false), "{status}");
    assert_eq!(
        status["rebuild_required"],
        serde_json::json!(false),
        "{status}"
    );
    assert!(
        status["auto_rebuilt"].is_null(),
        "auto-rebuild fired on a resolver mismatch: {status}"
    );
    assert!(
        has_stale_edge(&root),
        "status must not have rebuilt the store"
    );

    stamp_resolver(&root, None);
    let unrecorded = status_json(&root, &[]);
    assert!(
        unrecorded["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("unrecorded"),
        "an unstamped generation reads as unrecorded; status said: {unrecorded}"
    );
    assert_eq!(unrecorded["rebuild_required"], serde_json::json!(false));

    let aged = stage_store("status-older-extractor", true);
    let contrast = status_json(&aged, &[]);
    assert_eq!(
        contrast["rebuild_required"],
        serde_json::json!(true),
        "{contrast}"
    );
}
