//! A stacked lane can be provisioned by a build without the parsing frontend,
//! and the donor freshness check still refuses what it must.
//!
//! GitPulse links `devmap-query` and `devmap-store` with
//! `default-features = false` and provisions stacked lanes in-process. The
//! donor check used to bail "this build cannot confirm donor ... (parse
//! feature off)" there, because the question it asks of the store --
//! `latest_generation_matches_working_tree` -- was itself compiled only with
//! `parse`. Nothing in that question parses: it compares the stored payload
//! identities, the per-file content hashes and the refusal inventory with one
//! `scan_tree` and one `collect_project_manifests` of the donor, and neither of
//! those reaches a grammar. So every lane GitPulse asked for was refused.
//!
//! The generations here are written by SQL, because a build without grammars
//! writes none (the same reason `a_query_only_build_can_call_a_store_current`
//! gives). They are what a parsing build writes for the same tree: the content
//! hashes and refusal inventory come from the same scan, and the payload
//! identities from `current_payload_identity`. The parse-gated test at the end
//! builds the generation the real way and shows the two stores agree on every
//! scenario, so the SQL fixture cannot drift from the writer unnoticed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use devmap_extract::{collect_project_manifests, scan_tree, store_path};
use devmap_query::stacked::provision_stacked_worktree;
use devmap_store::{DiscoveryInventory, Store, GAP_DISCOVERY_REFUSED, GAP_OVER_LIMIT};
use rusqlite::{params, Connection};

const STALE: &str = "changed since its index was built";

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-donor-no-grammars-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn commit_all(root: &Path, message: &str) -> String {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
    git(root, &["rev-parse", "HEAD"])
}

/// The generation a parsing build writes for the tree as it is now, by SQL.
fn write_generation(root: &Path, head: &str) {
    let db = store_path(root);
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    drop(Store::open(&db).expect("a fresh store must open"));
    let scanned = scan_tree(root).unwrap();
    let manifests = collect_project_manifests(root).unwrap();
    let inventory = DiscoveryInventory::of(&scanned.report, &manifests);

    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
         VALUES (1, 1.0, ?1, ?2, '{\"total_files\":0,\"total_symbols\":0,
                 \"total_edges\":0,\"dead_symbols\":[],\"communities\":[],\"status\":\"Ok\"}')",
        params![head, root.to_string_lossy()],
    )
    .unwrap();
    for (index, (path, source)) in scanned.sources.iter().enumerate() {
        let id = index as i64 + 1;
        let language = devmap_extract::languages::detect_language(Path::new(path));
        let (grammar, analyzer) = devmap_extract::cache::current_payload_identity(language);
        conn.execute(
            "INSERT INTO paths (id, path) VALUES (?1, ?2)",
            params![id, path],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO file_payloads
                 (payload_id, file_id, content_hash, language, grammar_version,
                  analyzer_version, parse_outcome_json, engine_json, extraction_json)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, '\"Clean\"', '\"TreeSitter\"', '{}')",
            params![
                id,
                devmap_extract::content_hash(source) as i64,
                language,
                grammar,
                analyzer
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO file_rows (file_id, payload_id, valid_from, valid_to)
             VALUES (?1, ?1, 1, NULL)",
            params![id],
        )
        .unwrap();
    }
    for (gap, rows) in [
        (GAP_DISCOVERY_REFUSED, &inventory.refusals),
        (GAP_OVER_LIMIT, &inventory.over_limit),
    ] {
        for row in rows {
            conn.execute(
                "INSERT INTO generation_coverage_gaps (generation_id, gap, path, reason)
                 VALUES (1, ?1, ?2, ?3)",
                params![gap, row.path, row.reason],
            )
            .unwrap();
        }
    }
}

/// A committed donor whose latest generation was written from its tree.
fn clean_donor(label: &str) -> (PathBuf, PathBuf) {
    let root = tmp_dir(label).join("donor");
    fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    // As every indexed repository does: the store is state, never history.
    // Committed, the next commit would carry the store into `HEAD`, and a lane
    // checked out from it would already have one.
    let ignore: String = devmap_extract::paths::STATE_DIR_NAMES
        .iter()
        .map(|name| format!("{name}/\n"))
        .collect();
    fs::write(root.join(".gitignore"), ignore).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(root.join("src/b.rs"), "pub fn b() -> u8 { 2 }\n").unwrap();
    fs::write(root.join("src/c.ts"), "export const c = 3;\n").unwrap();
    let head = commit_all(&root, "one");
    write_generation(&root, &head);
    let lane = root.parent().unwrap().join("lane");
    (root, lane)
}

fn add_lane(donor: &Path, lane: &Path) {
    git(
        donor,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "lane",
            lane.to_str().unwrap(),
        ],
    );
}

/// Change the donor's tree, commit it, and restamp the generation to the new
/// `HEAD`: `HEAD` and the working tree agree, and only the index is stale.
fn restamp_after(root: &Path, mutate: impl FnOnce(&Path)) {
    mutate(root);
    let head = commit_all(root, "two");
    let conn = Connection::open(store_path(root)).unwrap();
    conn.execute("UPDATE generations SET head_sha = ?1", params![head])
        .unwrap();
}

fn provision_error(donor: &Path, lane: &Path) -> String {
    add_lane(donor, lane);
    format!(
        "{:#}",
        provision_stacked_worktree(donor, lane).expect_err("a stale donor must be refused")
    )
}

fn matches(root: &Path) -> bool {
    Store::open_read_only(store_path(root))
        .unwrap()
        .latest_generation_matches_working_tree()
        .unwrap()
}

#[test]
fn a_clean_donor_provisions_without_grammars() {
    let (donor, lane) = clean_donor("clean");
    assert!(
        matches(&donor),
        "fixture precondition: the index is current"
    );
    add_lane(&donor, &lane);
    let lease = provision_stacked_worktree(&donor, &lane).unwrap_or_else(|error| {
        panic!("a clean donor must provision in a build without grammars: {error:#}")
    });
    assert_eq!(lease.generation_id, 1);
    assert_eq!(lease.source_repo_root, donor.to_string_lossy());
}

#[test]
fn a_modified_file_refuses_the_donor_without_grammars() {
    let (donor, lane) = clean_donor("modified");
    restamp_after(&donor, |root| {
        fs::write(root.join("src/a.py"), "def a():\n    return 99\n").unwrap()
    });
    assert!(!matches(&donor));
    let error = provision_error(&donor, &lane);
    assert!(error.contains(STALE), "{error}");
}

#[test]
fn an_added_file_refuses_the_donor_without_grammars() {
    let (donor, lane) = clean_donor("added");
    restamp_after(&donor, |root| {
        fs::write(root.join("src/d.py"), "def d():\n    return 4\n").unwrap()
    });
    assert!(!matches(&donor));
    let error = provision_error(&donor, &lane);
    assert!(error.contains(STALE), "{error}");
}

#[test]
fn a_deleted_file_refuses_the_donor_without_grammars() {
    let (donor, lane) = clean_donor("deleted");
    restamp_after(&donor, |root| {
        fs::remove_file(root.join("src/b.rs")).unwrap()
    });
    assert!(!matches(&donor));
    let error = provision_error(&donor, &lane);
    assert!(error.contains(STALE), "{error}");
}

/// A broken manifest the resolver reads refuses the donor. `pyproject.toml` is
/// also a hashed source (every file with a detected language is), so both the
/// content hashes and the refusal inventory move; the parity test below shows
/// a built generation answers the same.
#[test]
fn a_changed_manifest_refuses_the_donor_without_grammars() {
    let (donor, lane) = clean_donor("manifest");
    restamp_after(&donor, |root| {
        fs::write(root.join("pyproject.toml"), "[project\nname = \n").unwrap();
        let manifests = collect_project_manifests(root).unwrap();
        assert!(
            manifests
                .refusals
                .iter()
                .any(|(path, _)| path == "pyproject.toml"),
            "fixture precondition: the manifest pass refuses it"
        );
    });
    assert!(!matches(&donor));
    let error = provision_error(&donor, &lane);
    assert!(error.contains(STALE), "{error}");
}

#[test]
fn an_uncommitted_change_refuses_the_donor_without_grammars() {
    let (donor, lane) = clean_donor("dirty");
    fs::write(donor.join("src/a.py"), "def a():\n    return 99\n").unwrap();
    let error = provision_error(&donor, &lane);
    assert!(error.contains("uncommitted path"), "{error}");
}

/// A generation that cannot be compared is refused, not waved through: the
/// donor's recorded root no longer exists, so the scan has nothing to read.
#[test]
fn a_donor_whose_tree_cannot_be_compared_is_refused_not_trusted() {
    let (donor, _) = clean_donor("unreadable");
    let conn = Connection::open(store_path(&donor)).unwrap();
    conn.execute(
        "UPDATE generations SET repo_root = ?1",
        params![donor.join("no-such-dir").to_string_lossy()],
    )
    .unwrap();
    drop(conn);
    let store = Store::open_read_only(store_path(&donor)).unwrap();
    assert!(
        store.latest_generation_matches_working_tree().is_err(),
        "an unscannable root is an error, which callers must treat as do-not-skip"
    );
}

/// With grammars, the generation is built the way `devmap build` builds it, and
/// the store that holds it answers each scenario exactly as the SQL-written
/// store does. Both feature builds therefore give one answer per fixture.
#[cfg(feature = "parse")]
mod parity {
    use super::*;
    use devmap_store::GenerationWriteOpts;

    fn real_generation(root: &Path, head: &str) -> Store {
        let (files, _report) = devmap_extract::extract_tree_with_report(root).unwrap();
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&files);
        let resolution = resolver.resolve_all(&files).unwrap();
        let analysis = devmap_analyze::analyze(&files, &resolution);
        let store = Store::open_in_memory().unwrap();
        store
            .save_generation_with_metadata(
                &files,
                &resolution,
                &analysis,
                GenerationWriteOpts {
                    repo_root: Some(root.to_string_lossy().into_owned()),
                    ..GenerationWriteOpts::default()
                },
                head,
            )
            .unwrap();
        store
    }

    #[test]
    fn a_built_generation_and_a_hand_written_one_agree_on_every_scenario() {
        type Mutation = fn(&Path);
        let scenarios: [(&str, Mutation, bool); 5] = [
            ("untouched", |_| {}, true),
            (
                "modified",
                |root| fs::write(root.join("src/a.py"), "def a():\n    return 99\n").unwrap(),
                false,
            ),
            (
                "added",
                |root| fs::write(root.join("src/d.py"), "def d():\n    return 4\n").unwrap(),
                false,
            ),
            (
                "deleted",
                |root| fs::remove_file(root.join("src/b.rs")).unwrap(),
                false,
            ),
            (
                "manifest",
                |root| fs::write(root.join("pyproject.toml"), "[project\nname = \n").unwrap(),
                false,
            ),
        ];
        for (name, mutate, expected) in scenarios {
            let (donor, _) = clean_donor(&format!("parity-{name}"));
            let head = git(&donor, &["rev-parse", "HEAD"]);
            let built = real_generation(&donor, &head);
            mutate(&donor);
            let by_sql = matches(&donor);
            let by_build = built.latest_generation_matches_working_tree().unwrap();
            assert_eq!(
                by_sql, by_build,
                "{name}: the hand-written generation answers {by_sql}, the built one {by_build}"
            );
            assert_eq!(by_build, expected, "{name}");
        }
    }
}
