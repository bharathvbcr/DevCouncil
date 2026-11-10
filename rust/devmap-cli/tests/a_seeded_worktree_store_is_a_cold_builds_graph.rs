//! `devmap build --seed-from <main>` starts a linked worktree's store from its
//! main checkout's and then builds incrementally.
//!
//! What has to hold, and what each test breaks if it does not:
//!
//! * the seeded store answers the same graph as a cold build of the same tree
//!   (only the generation number differs), is its own inode, and names the
//!   worktree as its owner, not the main checkout;
//! * only the worktree's registered main checkout is an accepted source, and a
//!   refused one leaves the worktree with no store, so the next build is cold
//!   and holds nothing of the refused repository;
//! * a source whose WAL holds committed pages is read through SQLite, so the
//!   seed carries them;
//! * the seed also carries the main checkout's manifest artifacts, and the
//!   seeding build rewrites them for the worktree even without `--manifest`
//!   (the hook's form), so the manifest writer patches clones of the main
//!   checkout's files instead of laying fresh ones down.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use serde_json::Value;

struct Fixture {
    base: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "devmap-seed-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let fixture = Self { base };
        let main = fixture.main();
        fs::create_dir_all(main.join("src")).unwrap();
        fs::write(
            main.join("src/a.py"),
            "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n",
        )
        .unwrap();
        fs::write(
            main.join("src/b.py"),
            "from a import alpha\n\n\ndef gamma():\n    return alpha()\n",
        )
        .unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "fixture"]);
        for lane in ["lane", "cold", "sibling"] {
            git(
                &main,
                &["worktree", "add", "-q", "--detach", &format!("../{lane}")],
            );
        }
        // An unrelated repository with its own symbol and its own store.
        let other = fixture.base.join("other");
        fs::create_dir_all(other.join("src")).unwrap();
        fs::write(
            other.join("src/o.py"),
            "def unrelated_marker():\n    return 0\n",
        )
        .unwrap();
        git(&other, &["init", "-q"]);
        git(&other, &["add", "."]);
        git(&other, &["commit", "-qm", "other"]);
        fixture
    }

    fn main(&self) -> PathBuf {
        self.base.join("main")
    }

    fn at(&self, name: &str) -> PathBuf {
        self.base.join(name).canonicalize().unwrap()
    }

    fn devmap(&self, root: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_devmap"))
            .arg("--root")
            .arg(root)
            .args(args)
            .current_dir(&self.base)
            .env_remove("DEVMAP_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("DEVMAP_AUTOSPAWN", "0")
            .env("TOKIO_WORKER_THREADS", "2")
            .env("RAYON_NUM_THREADS", "2")
            .output()
            .unwrap()
    }

    fn build_main(&self) {
        let out = self.devmap(&self.main(), &["build", "--manifest"]);
        assert!(out.status.success(), "{}", stderr(&out));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=",
            "-c",
            "init.templateDir=",
        ])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn store(root: &Path) -> PathBuf {
    root.join(".devmap/codeintel/devmap.sqlite")
}

/// The manifest's code graph with the one field a cold and a seeded build
/// legitimately disagree on, the generation they ended on, removed.
fn graph(root: &Path) -> Value {
    let text = fs::read_to_string(root.join(".devmap/graph/code_graph.json")).unwrap();
    let mut graph: Value = serde_json::from_str(&text).unwrap();
    graph["meta"]["devmap_rust"]
        .as_object_mut()
        .expect("meta.devmap_rust")
        .remove("generation_id")
        .expect("a generation id to mask");
    graph
}

fn scalar<T: rusqlite::types::FromSql>(root: &Path, sql: &str) -> T {
    Connection::open(store(root))
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

/// The same uncommitted edit in two worktrees of one commit, so the seeded
/// build has real incremental work and the cold build has the same tree.
fn edit(root: &Path) {
    let a = root.join("src/a.py");
    let mut text = fs::read_to_string(&a).unwrap();
    text.push_str("\n\ndef delta():\n    return alpha()\n");
    fs::write(a, text).unwrap();
}

const VERBOSE: [&str; 3] = ["--progress", "always", "--verbose"];

fn seeded_build(fx: &Fixture, root: &Path, from: &Path) -> Output {
    let mut args = VERBOSE.to_vec();
    args.extend(["build", "--manifest", "--seed-from", from.to_str().unwrap()]);
    fx.devmap(root, &args)
}

#[test]
fn a_seeded_store_is_the_graph_of_a_cold_build_owned_by_its_worktree() {
    let fx = Fixture::new();
    fx.build_main();
    let (lane, cold) = (fx.at("lane"), fx.at("cold"));
    edit(&lane);
    edit(&cold);

    let out = seeded_build(&fx, &lane, &fx.main());
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("by clone") || stderr(&out).contains("by copy"),
        "the seed was not taken: {}",
        stderr(&out)
    );
    let cold_out = fx.devmap(&cold, &["build", "--manifest"]);
    assert!(cold_out.status.success(), "{}", stderr(&cold_out));

    assert_eq!(
        graph(&lane),
        graph(&cold),
        "the seeded graph differs from a cold build's"
    );
    // The seed carried the main checkout's generation, so the incremental
    // build is the second one; a cold build is the first.
    assert_eq!(scalar::<i64>(&lane, "SELECT max(id) FROM generations"), 2);
    assert_eq!(scalar::<i64>(&cold, "SELECT max(id) FROM generations"), 1);

    let lane_text = lane.to_str().unwrap();
    assert_eq!(
        scalar::<String>(
            &lane,
            "SELECT repo_root FROM pending_state WHERE singleton = 1"
        ),
        lane_text
    );
    assert_eq!(
        scalar::<i64>(
            &lane,
            &format!("SELECT count(*) FROM generations WHERE repo_root IS NOT '{lane_text}'")
        ),
        0,
        "a generation still names another worktree"
    );
    // The main checkout's store was only read.
    let main_text = fx.at("main");
    assert_eq!(
        scalar::<String>(
            &main_text,
            "SELECT repo_root FROM pending_state WHERE singleton = 1"
        ),
        main_text.to_str().unwrap()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(store(&lane)).unwrap().nlink(), 1);
    }
    let leftovers: Vec<_> = fs::read_dir(store(&lane).parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".seed-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging files left behind: {leftovers:?}"
    );

    // The seeded store is an ordinary store: the next build is a no-op.
    let again = fx.devmap(&lane, &["build"]);
    assert!(again.status.success(), "{}", stderr(&again));
}

#[test]
fn only_the_registered_main_checkout_may_seed_a_worktree() {
    let fx = Fixture::new();
    fx.build_main();
    let other = fx.at("other");
    let built = fx.devmap(&other, &["build", "--manifest"]);
    assert!(built.status.success(), "{}", stderr(&built));
    let lane = fx.at("lane");

    // A forged gitlink: a directory whose `.git` names the lane's registration.
    let evil = fx.base.join("evil");
    fs::create_dir_all(&evil).unwrap();
    fs::write(
        evil.join(".git"),
        format!(
            "gitdir: {}\n",
            fx.main().join(".git/worktrees/lane").display()
        ),
    )
    .unwrap();
    let evil = evil.canonicalize().unwrap();

    let refusals: [(&Path, PathBuf); 4] = [
        (&lane, other.clone()),
        (&lane, fx.at("sibling")),
        (&lane, lane.clone()),
        (&evil, fx.at("main")),
    ];
    for (target, from) in &refusals {
        let out = seeded_build(&fx, target, from);
        assert!(
            !out.status.success(),
            "seeding {target:?} from {from:?} was accepted"
        );
        assert!(stderr(&out).contains("--seed-from"), "{}", stderr(&out));
        assert!(
            !target.join(".devmap").exists() && !target.join(".devcouncil").exists(),
            "a refused seed left state in {target:?}"
        );
    }

    // Nothing of the refused repository is in what the cold build makes.
    let cold = fx.devmap(&lane, &["build", "--manifest"]);
    assert!(cold.status.success(), "{}", stderr(&cold));
    let graph = fs::read_to_string(lane.join(".devmap/graph/code_graph.json")).unwrap();
    assert!(
        !graph.contains("unrelated_marker"),
        "the other repository's symbol was seeded"
    );
    assert!(graph.contains("alpha"));
    assert_eq!(scalar::<i64>(&lane, "SELECT max(id) FROM generations"), 1);
}

#[test]
fn a_source_with_a_live_wal_is_read_through_sqlite() {
    let fx = Fixture::new();
    fx.build_main();
    let (lane, cold) = (fx.at("lane"), fx.at("cold"));

    // A writer that never checkpoints: its commit lives only in the WAL, so a
    // copy of the database file alone would not contain it.
    let live = Connection::open(store(&fx.main())).unwrap();
    live.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    live.execute("UPDATE generations SET created_at = 1234567890", [])
        .unwrap();
    let wal = fx.main().join(".devmap/codeintel/devmap.sqlite-wal");
    assert!(
        fs::metadata(&wal).is_ok_and(|meta| meta.len() > 0),
        "the fixture failed to leave committed pages in the WAL"
    );

    let out = seeded_build(&fx, &lane, &fx.main());
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("by backup"), "{}", stderr(&out));
    drop(live);

    assert_eq!(
        scalar::<String>(&lane, "PRAGMA integrity_check"),
        "ok",
        "the seeded store is damaged"
    );
    assert!(
        scalar::<i64>(
            &lane,
            "SELECT count(*) FROM generations WHERE created_at = 1234567890"
        ) >= 1,
        "the WAL's committed page did not reach the seed"
    );
    assert!(!lane.join(".devmap/codeintel/devmap.sqlite-shm").exists());
    let cold_out = fx.devmap(&cold, &["build", "--manifest"]);
    assert!(cold_out.status.success(), "{}", stderr(&cold_out));
    assert_eq!(graph(&lane), graph(&cold));
}

#[test]
fn a_seed_carries_the_manifest_artifacts_and_rewrites_them_for_the_worktree() {
    let fx = Fixture::new();
    fx.build_main();
    let (lane, cold) = (fx.at("lane"), fx.at("cold"));
    edit(&lane);
    edit(&cold);

    // The hook's form: `build --seed-from`, no `--manifest`.
    let main = fx.main();
    let mut args = VERBOSE.to_vec();
    args.extend(["build", "--seed-from", main.to_str().unwrap()]);
    let out = fx.devmap(&lane, &args);
    assert!(out.status.success(), "{}", stderr(&out));
    let said = stderr(&out);
    assert!(
        said.contains("code_graph.json by clone") || said.contains("code_graph.json by copy"),
        "the main checkout's code graph was not seeded: {said}"
    );
    assert!(
        lane.join(".devmap/repo_map.json").is_file(),
        "the repo map was not seeded"
    );

    // Rewritten for this worktree in the same build, not left describing the
    // main checkout: it is the graph a cold manifest build of the same tree
    // writes.
    let cold_out = fx.devmap(&cold, &["build", "--manifest"]);
    assert!(cold_out.status.success(), "{}", stderr(&cold_out));
    assert_eq!(
        graph(&lane),
        graph(&cold),
        "the seeded artifacts still describe the main checkout"
    );
    let leftovers: Vec<_> = fs::read_dir(lane.join(".devmap/graph"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".seed-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging files left behind: {leftovers:?}"
    );
}

#[test]
fn devmap_seed_supports_sibling_checkouts_clones_reuse_cache_and_refuses_forgery() {
    let fx = Fixture::new();
    fx.build_main();
    let (lane, sibling, other) = (fx.at("lane"), fx.at("sibling"), fx.at("other"));
    assert!(fx.devmap(&other, &["build", "--manifest"]).status.success());

    // Forge a gitlink pointing at `lane`'s registration.
    let evil = fx.base.join("evil-seed");
    fs::create_dir_all(&evil).unwrap();
    fs::write(
        evil.join(".git"),
        format!(
            "gitdir: {}\n",
            fx.main().join(".git/worktrees/lane").display()
        ),
    )
    .unwrap();
    let evil = evil.canonicalize().unwrap();

    // Unrelated repo and forged gitlink must fail closed.
    assert!(!fx
        .devmap(
            &lane,
            &["--json", "seed", "--from", other.to_str().unwrap()]
        )
        .status
        .success());
    assert!(!fx
        .devmap(
            &evil,
            &["--json", "seed", "--from", fx.main().to_str().unwrap()]
        )
        .status
        .success());

    // Seed `sibling` from `main` (default), then seed `lane` from `sibling`!
    let sib_seed = fx.devmap(&sibling, &["--json", "seed"]);
    assert!(sib_seed.status.success(), "{}", stderr(&sib_seed));
    let lane_seed = fx.devmap(
        &lane,
        &["--json", "seed", "--from", sibling.to_str().unwrap()],
    );
    assert!(lane_seed.status.success(), "{}", stderr(&lane_seed));

    let status_out = fx.devmap(&lane, &["--json", "status"]);
    assert!(status_out.status.success(), "{}", stderr(&status_out));
    let st: Value = serde_json::from_slice(&status_out.stdout).unwrap();
    assert_eq!(st["query_ready"], Value::Bool(true), "{st}");

    // Verify `resolution-reuse-*` was cloned if `main` wrote one (`F9`).
    let main_reuse: Vec<_> = fs::read_dir(fx.main().join(".devmap"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("resolution-reuse-")
        })
        .collect();
    if !main_reuse.is_empty() {
        let lane_reuse: Vec<_> = fs::read_dir(lane.join(".devmap"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("resolution-reuse-")
            })
            .collect();
        assert_eq!(lane_reuse.len(), main_reuse.len());
    }
}

#[test]
fn devmap_seed_stacked_eliminates_split_brain_in_paths_and_status_and_fails_closed_on_corruption() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    edit(&lane);

    let seed_out = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(seed_out.status.success(), "{}", stderr(&seed_out));
    let seed_json: Value = serde_json::from_slice(&seed_out.stdout).unwrap();
    assert_eq!(seed_json["mode"], "stacked");
    assert_eq!(seed_json["synced_files"], 1);

    let paths_out = fx.devmap(&lane, &["--json", "paths"]);
    assert!(paths_out.status.success(), "{}", stderr(&paths_out));
    let paths_json: Value = serde_json::from_slice(&paths_out.stdout).unwrap();
    assert_eq!(paths_json["store_exists"], true);
    assert_eq!(paths_json["sqlite_store_exists"], false);
    assert_eq!(paths_json["stacked_exists"], true);

    let status_out = fx.devmap(&lane, &["--json", "status"]);
    assert!(status_out.status.success(), "{}", stderr(&status_out));
    let status_json: Value = serde_json::from_slice(&status_out.stdout).unwrap();
    // `query_ready` is derived from the schema relation, the generation and the
    // search index (`host_contract_fields`), never from `is_fresh`, so the base
    // stays queryable. `is_fresh` is the base's own verdict against the lane's
    // tree: the lane has an edit the base does not hold, so it is NOT fresh, and
    // `stacked.dirty_files` is what tells the reader a delta covers it.
    assert_eq!(status_json["query_ready"], true, "{status_json}");
    assert_eq!(status_json["is_fresh"], false, "{status_json}");
    assert_eq!(status_json["stacked"]["base_generation_id"], 1);
    assert_eq!(status_json["stacked"]["dirty_files"], 1);

    // Corrupt `stack_lease.json`: `status` must fail closed (`query_ready: false`)!
    let lease_file = lane.join(".devmap/codeintel/stack_lease.json");
    fs::write(&lease_file, "{corrupt").unwrap();
    let bad_status = fx.devmap(&lane, &["--json", "status"]);
    assert!(bad_status.status.success());
    let bad_json: Value = serde_json::from_slice(&bad_status.stdout).unwrap();
    assert_eq!(bad_json["query_ready"], false, "{bad_json}");
    assert!(
        bad_json["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("stacked worktree store unreadable"),
        "{bad_json}"
    );
}

#[test]
fn standalone_seed_places_the_store_last_and_nothing_after_it_can_fail() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    // A regular file where the code-graph artifact's directory must go: seeding
    // `.devmap/graph/code_graph.json` fails, after the store has been placed.
    let state = lane.join(".devmap");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("graph"), "not a directory").unwrap();

    let out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(
        out.status.success(),
        "an artifact that cannot be seeded must not fail a command that already placed the store; stderr: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("derived artifacts were not seeded"),
        "the skipped artifacts must be named: {}",
        stderr(&out)
    );
    let payload: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(payload["mode"], "cow");
    assert!(lane.join(".devmap/codeintel/devmap.sqlite").is_file());
    let leftovers: Vec<_> = fs::read_dir(lane.join(".devmap/codeintel"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".seed-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staged files left behind: {leftovers:?}"
    );
}

#[test]
fn standalone_seed_from_an_unreadable_donor_leaves_no_store() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    let donor_db = fx.main().join(".devmap/codeintel/devmap.sqlite");
    fs::write(&donor_db, b"this is not a sqlite database").unwrap();

    let out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(!out.status.success(), "a corrupt donor must fail the seed");
    assert!(
        !lane.join(".devmap/codeintel/devmap.sqlite").exists(),
        "a failed seed must leave no store in place"
    );
    let leftovers: Vec<_> = fs::read_dir(lane.join(".devmap/codeintel"))
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(
        leftovers
            .iter()
            .all(|e| !e.file_name().to_string_lossy().contains(".seed-")),
        "staged files left behind: {leftovers:?}"
    );
}

#[test]
fn stacked_seed_refuses_a_dirty_donor() {
    let fx = Fixture::new();
    fx.build_main();
    // Edit the donor's checkout and do NOT rebuild it: its index no longer
    // describes HEAD's tree, so a stacked base taken from it would serve the
    // uncommitted edit as the lane's own.
    edit(&fx.main());
    let lane = fx.at("lane");

    let out = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(
        !out.status.success(),
        "a stacked seed from a dirty donor must fail; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let err = stderr(&out);
    assert!(
        err.contains("uncommitted") && err.contains(fx.main().to_str().unwrap()),
        "stderr must name the dirty donor: {err}"
    );
    assert!(
        !lane.join(".devmap/codeintel/stack_lease.json").exists(),
        "a refused seed must leave no lease behind"
    );
}

#[test]
fn seeded_build_writes_manifest_even_when_main_has_no_manifest_artifacts() {
    let fx = Fixture::new();
    // Build `main` WITHOUT `--manifest` (`F2` reproduction)!
    let build_only = fx.devmap(&fx.main(), &["build"]);
    assert!(build_only.status.success(), "{}", stderr(&build_only));
    assert!(!fx.main().join(".devmap/repo_map.json").exists());

    let lane = fx.at("lane");
    let main = fx.main();
    let out = fx.devmap(&lane, &["build", "--seed-from", main.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        lane.join(".devmap/repo_map.json").is_file(),
        "seeded worktree lacked repo_map.json when main omitted --manifest"
    );
    assert!(
        lane.join(".devmap/graph/code_graph.json").is_file(),
        "seeded worktree lacked code_graph.json when main omitted --manifest"
    );
}

#[test]
fn standalone_seed_builds_lane_edits_and_stamps_lane_head_before_atomic_placement() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");

    // Advance `lane` with its own committed symbol so `lane` HEAD != `main` HEAD.
    fs::write(
        lane.join("src/a.py"),
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n\n\ndef lane_committed_only():\n    return beta()\n",
    )
    .unwrap();
    git(&lane, &["add", "."]);
    git(&lane, &["commit", "-qm", "lane-diverged"]);
    let lane_head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&lane)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    // Run standalone `devmap seed` WITHOUT running `devmap build` afterwards (`F-g`).
    let seed_out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(seed_out.status.success(), "{}", stderr(&seed_out));
    let seed_json: Value = serde_json::from_slice(&seed_out.stdout).unwrap();
    assert_eq!(seed_json["mode"], "cow");

    // Immediately queryable for `lane_committed_only`, fresh, and stamped with `lane`'s HEAD.
    let search_out = fx.devmap(&lane, &["--json", "search", "--", "lane_committed_only"]);
    assert!(search_out.status.success(), "{}", stderr(&search_out));
    let search_json: Value = serde_json::from_slice(&search_out.stdout).unwrap();
    assert_eq!(
        search_json["shown"], 1,
        "standalone seed must index lane's own edits before placement: {search_json}"
    );

    let status_out = fx.devmap(&lane, &["--json", "status"]);
    assert!(status_out.status.success(), "{}", stderr(&status_out));
    let status_json: Value = serde_json::from_slice(&status_out.stdout).unwrap();
    assert_eq!(status_json["is_fresh"], true, "{status_json}");
    assert_eq!(status_json["query_ready"], true, "{status_json}");
    assert!(
        status_json["generation_id"].as_i64().unwrap_or(0) >= 2,
        "expected generation +1 for diverged lane: {status_json}"
    );

    let recorded_head: String = scalar(
        &lane,
        "SELECT head_sha FROM generations ORDER BY id DESC LIMIT 1",
    );
    assert_eq!(recorded_head, lane_head);
    assert!(
        lane.join(".devmap/codeintel/devmap.sqlite.csr").is_file(),
        "CSR sidecar must be materialized alongside placed store"
    );

    // Simulated build failure on staged store (`DROP TABLE nodes_fts_data` in donor + diverged
    // `sibling` tree) must fail before placement and leave zero store or staged files in `sibling`.
    let donor_db = fx.main().join(".devmap/codeintel/devmap.sqlite");
    {
        let conn = Connection::open(&donor_db).unwrap();
        conn.execute_batch("DROP TABLE nodes_fts_data;").unwrap();
    }
    let sibling = fx.at("sibling");
    edit(&sibling);
    let failed_seed = fx.devmap(&sibling, &["--json", "seed"]);
    assert!(
        !failed_seed.status.success(),
        "staged build failure must fail devmap seed; stdout: {}",
        String::from_utf8_lossy(&failed_seed.stdout)
    );
    assert!(
        !sibling.join(".devmap/codeintel/devmap.sqlite").exists(),
        "failed staged build must never place partial devmap.sqlite"
    );
    let leftovers: Vec<_> = fs::read_dir(sibling.join(".devmap/codeintel"))
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(
        leftovers
            .iter()
            .all(|e| !e.file_name().to_string_lossy().contains(".seed-")),
        "staged files left behind after build failure: {leftovers:?}"
    );
}

#[test]
fn seeded_build_and_standalone_seed_guard_csr_generation_dangling_symlinks_and_orphaned_wal() {
    let fx = Fixture::new();
    fx.build_main();
    let main = fx.main();
    // Materialize a donor CSR at generation 1 on `main`.
    {
        let donor_store = devmap_store::Store::open(main.join(".devmap/codeintel/devmap.sqlite"))
            .expect("open donor store");
        let csr_path = donor_store
            .write_latest_csr_sidecar()
            .expect("write donor csr")
            .expect("donor csr path");
        let reader = devmap_store::csr::CsrGraphReader::open_mmap(&csr_path).unwrap();
        assert_eq!(reader.generation() as i64, 1);
    }

    // 1. `devmap build --seed-from <main>` on a diverged lane builds generation >= 2:
    // its `.csr` sidecar MUST match the lane's new generation, NOT stale donor generation 1!
    let diverged = fx.at("lane");
    edit(&diverged);
    let build_out = fx.devmap(&diverged, &["build", "--seed-from", main.to_str().unwrap()]);
    assert!(build_out.status.success(), "{}", stderr(&build_out));
    let diverged_gen: i64 = scalar(
        &diverged,
        "SELECT id FROM generations ORDER BY id DESC LIMIT 1",
    );
    assert!(
        diverged_gen >= 2,
        "expected generation >= 2, got {diverged_gen}"
    );
    let diverged_csr = diverged.join(".devmap/codeintel/devmap.sqlite.csr");
    assert!(
        diverged_csr.is_file(),
        "diverged --seed-from build must write fresh CSR sidecar"
    );
    let csr_reader = devmap_store::csr::CsrGraphReader::open_mmap(&diverged_csr).unwrap();
    assert_eq!(
        csr_reader.generation() as i64,
        diverged_gen,
        "diverged --seed-from must not overwrite lane CSR with stale donor generation 1 CSR"
    );

    // 2. Re-running `devmap seed` on `diverged` (where valid `devmap.sqlite` + `.csr` exist)
    // must fail closed without clobbering either file's bytes!
    let db_bytes_before = fs::read(diverged.join(".devmap/codeintel/devmap.sqlite")).unwrap();
    let csr_bytes_before = fs::read(&diverged_csr).unwrap();
    let reseed_out = fx.devmap(&diverged, &["--json", "seed"]);
    assert!(
        !reseed_out.status.success(),
        "re-seeding over existing store must fail"
    );
    assert_eq!(
        fs::read(diverged.join(".devmap/codeintel/devmap.sqlite")).unwrap(),
        db_bytes_before,
        "refused seed must not clobber existing devmap.sqlite"
    );
    assert_eq!(
        fs::read(&diverged_csr).unwrap(),
        csr_bytes_before,
        "refused seed must not clobber existing devmap.sqlite.csr"
    );

    // 3. Dangling symlink at `target_db` must be refused without unlinking or replacing it!
    #[cfg(unix)]
    {
        let symlink_lane = fx.at("cold");
        let ci_dir = symlink_lane.join(".devmap/codeintel");
        fs::create_dir_all(&ci_dir).unwrap();
        let sym_db = ci_dir.join("devmap.sqlite");
        std::os::unix::fs::symlink(ci_dir.join("nonexistent-target.sqlite"), &sym_db).unwrap();
        let sym_out = fx.devmap(&symlink_lane, &["--json", "seed"]);
        assert!(
            !sym_out.status.success(),
            "dangling symlink at target_db must be refused"
        );
        assert!(
            fs::symlink_metadata(&sym_db)
                .unwrap()
                .file_type()
                .is_symlink(),
            "refused seed must leave existing symlink intact"
        );
    }

    // 4. Orphaned `-wal` / `-shm` files sitting in `.devmap/codeintel` (with no `devmap.sqlite`)
    // must be swept under the writer lock before `devmap seed` places `devmap.sqlite`,
    // and CSR sidecar write failures must surface as errors without placing a store!
    let orphan_lane = fx.at("sibling");
    let orphan_ci = orphan_lane.join(".devmap/codeintel");
    fs::create_dir_all(&orphan_ci).unwrap();
    fs::write(
        orphan_ci.join("devmap.sqlite-wal"),
        b"GARBAGE_ORPHAN_WAL_FRAMES",
    )
    .unwrap();
    fs::write(orphan_ci.join("devmap.sqlite-shm"), b"GARBAGE_ORPHAN_SHM").unwrap();

    // Block final CSR placement by placing a directory at `devmap.sqlite.csr`:
    // `devmap seed` must surface the error and NOT leave a partial `devmap.sqlite`!
    fs::create_dir_all(orphan_ci.join("devmap.sqlite.csr")).unwrap();
    let blocked_csr = fx.devmap(&orphan_lane, &["--json", "seed"]);
    assert!(
        !blocked_csr.status.success(),
        "CSR placement failure must fail devmap seed"
    );
    assert!(
        !orphan_ci.join("devmap.sqlite").exists(),
        "failed CSR placement must not leave devmap.sqlite behind"
    );
    fs::remove_dir_all(orphan_ci.join("devmap.sqlite.csr")).unwrap();

    // Re-plant orphaned WAL/SHM and verify clean `devmap seed` sweeps them and opens cleanly.
    fs::write(
        orphan_ci.join("devmap.sqlite-wal"),
        b"GARBAGE_ORPHAN_WAL_FRAMES",
    )
    .unwrap();
    fs::write(orphan_ci.join("devmap.sqlite-shm"), b"GARBAGE_ORPHAN_SHM").unwrap();
    let clean_seed = fx.devmap(&orphan_lane, &["--json", "seed"]);
    assert!(clean_seed.status.success(), "{}", stderr(&clean_seed));
    let search_clean = fx.devmap(&orphan_lane, &["--json", "search", "--", "alpha"]);
    assert!(search_clean.status.success(), "{}", stderr(&search_clean));
}

fn generation_rows(root: &Path) -> Vec<(i64, String, String)> {
    let conn = Connection::open(store(root)).unwrap();
    let mut stmt = conn
        .prepare("SELECT id, repo_root, head_sha FROM generations ORDER BY id")
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn head_of(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A `.csr` that outlived its store is another generation's graph. GitPulse maps
/// whichever sidecar sits beside the store and checks only the generation
/// number, so placing a store beside it without sweeping it is a silent wrong
/// answer. The donor here has no CSR (a plain `build` never writes one), which is
/// the usual case for a hook-driven `build --seed-from`.
#[test]
fn a_stale_csr_beside_an_absent_store_is_swept_when_the_seeded_store_brings_none() {
    let fx = Fixture::new();
    fx.build_main();
    let main = fx.main();
    assert!(
        !main.join(".devmap/codeintel/devmap.sqlite.csr").exists(),
        "fixture precondition: the donor has no CSR sidecar"
    );
    let lane = fx.at("lane");
    let ci = lane.join(".devmap/codeintel");
    fs::create_dir_all(&ci).unwrap();
    let stale = ci.join("devmap.sqlite.csr");
    fs::write(&stale, b"A-CSR-OF-ANOTHER-GENERATION").unwrap();

    let out = fx.devmap(&lane, &["build", "--seed-from", main.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(store(&lane).is_file(), "the seeded store was placed");
    assert!(
        !stale.exists(),
        "a store was placed beside a sidecar it did not write"
    );
}

/// The standalone seed of a lane whose tree equals the donor's but whose HEAD
/// does not (an empty commit). The graph is the donor's, so no generation is
/// written; only the stamp moves. Historical generations stay as the donor wrote
/// them (CoW-shared, `seed_rebind` binds the latest row), which a normalising
/// restamp would undo.
#[test]
fn a_standalone_seed_at_another_head_stamps_the_lane_head_and_leaves_history_untouched() {
    let fx = Fixture::new();
    fx.build_main();
    let main = fx.main();
    let full = fx.devmap(&main, &["build", "--full"]);
    assert!(full.status.success(), "{}", stderr(&full));
    let donor_rows = generation_rows(&main);
    assert!(
        donor_rows.len() >= 2,
        "fixture precondition: the donor holds a historical generation: {donor_rows:?}"
    );

    let lane = fx.at("lane");
    git(
        &lane,
        &["commit", "-q", "--allow-empty", "-m", "lane-moved"],
    );
    let lane_head = head_of(&lane);
    assert_ne!(lane_head, head_of(&main), "the fixture must move HEAD");

    let out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let rows = generation_rows(&lane);
    assert_eq!(
        rows.len(),
        donor_rows.len(),
        "a tree that matches the donor writes no generation"
    );
    let (latest, history) = rows.split_last().unwrap();
    assert_eq!(
        latest.2, lane_head,
        "the latest generation is stamped with the lane HEAD"
    );
    assert_eq!(
        latest.1,
        lane.to_str().unwrap(),
        "the latest generation is bound to the lane"
    );
    assert_eq!(
        history,
        &donor_rows[..donor_rows.len() - 1],
        "historical generations must stay byte-identical to the donor's"
    );
    let rebind: i64 = scalar(&lane, "SELECT count(*) FROM seed_rebind");
    assert_eq!(rebind, 1, "the seed rebind outlives the stamp");
}

/// A diverged standalone seed builds the lane's edits, so the donor's artifacts
/// no longer describe it. The lane must still not lack a manifest (spec F2), the
/// same as a seeded build.
#[test]
fn a_diverged_standalone_seed_writes_the_consumer_artifacts() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    edit(&lane);

    let out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let paths_out = fx.devmap(&lane, &["--json", "paths"]);
    assert!(paths_out.status.success(), "{}", stderr(&paths_out));
    let paths: Value = serde_json::from_slice(&paths_out.stdout).unwrap();
    for key in ["repo_map", "code_graph"] {
        let path = PathBuf::from(
            paths[key]
                .as_str()
                .unwrap_or_else(|| panic!("{key}: {paths}")),
        );
        assert!(
            path.starts_with(&lane),
            "{key} resolves into the lane: {path:?}"
        );
        assert!(
            path.is_file(),
            "a diverged seed left the lane without {key}: {path:?}"
        );
    }
    let graph = graph(&lane);
    let text = graph.to_string();
    assert!(
        text.contains("delta"),
        "the lane's graph describes the lane's edit, not the donor's tree"
    );
}

/// The staged build takes its own writer lock (`<target>.seed-<pid>.writer.lock`).
/// A build that fails after taking it must not leave that file behind: each
/// failed seed would leave one more. The failure here is injected after the lock
/// exists: a directory where the lane's code graph goes, so extraction,
/// resolution and the lock all happen and the manifest write then fails.
#[test]
fn a_staged_build_that_fails_after_taking_its_lock_leaves_no_staged_file_or_lock() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    edit(&lane);
    fs::create_dir_all(lane.join(".devmap/graph/code_graph.json")).unwrap();

    let out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(
        !out.status.success(),
        "a staged build that cannot write its manifest must fail the seed"
    );
    assert!(
        stderr(&out).contains("managed file is not a regular file"),
        "the failure came from the manifest write, after the staged lock was taken: {}",
        stderr(&out)
    );
    assert!(
        !store(&lane).exists(),
        "a failed seed must not place a store"
    );
    let leftovers: Vec<String> = fs::read_dir(lane.join(".devmap/codeintel"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".seed-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staged files left behind after a failed staged build: {leftovers:?}"
    );
}

fn seed_leftovers(lane: &Path) -> Vec<String> {
    fs::read_dir(lane.join(".devmap/codeintel"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".seed-"))
        .collect()
}

/// A diverged standalone seed writes its manifest while the store is still at
/// the staged path, and the artifact stamp (`<db>.artifacts.json`) follows the
/// store it was written beside. It must end up beside the placed store, and
/// nothing may remain at the staged path.
#[test]
fn a_diverged_standalone_seed_places_its_artifact_stamp_beside_the_store() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    edit(&lane);

    let out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert_eq!(
        seed_leftovers(&lane),
        Vec::<String>::new(),
        "a successful seed left staged files behind"
    );
    assert!(
        lane.join(".devmap/codeintel/devmap.sqlite.artifacts.json")
            .is_file(),
        "the placed store has no artifact stamp, so the next manifest regenerates"
    );
}

/// The failure that follows the stamp: the build and its manifest succeed, then
/// placement fails (a directory at the lane's `.csr`). The staged stamp has to be
/// discarded with the rest of the staged set.
#[test]
fn a_seed_that_fails_at_placement_discards_the_staged_artifact_stamp() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    edit(&lane);
    let ci = lane.join(".devmap/codeintel");
    fs::create_dir_all(ci.join("devmap.sqlite.csr")).unwrap();

    let out = fx.devmap(&lane, &["--json", "seed"]);
    assert!(
        !out.status.success(),
        "placement over a directory must fail the seed"
    );
    assert!(
        !store(&lane).exists(),
        "a failed seed must not place a store"
    );
    assert_eq!(
        seed_leftovers(&lane),
        Vec::<String>::new(),
        "staged files left behind after a failed placement"
    );
}

/// Every regular file under `dir`: relative path, length, mtime and bytes.
fn state_snapshot(
    dir: &Path,
) -> std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime, Vec<u8>)> {
    let mut out = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                pending.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().to_path_buf();
                out.insert(
                    rel,
                    (
                        meta.len(),
                        meta.modified().unwrap(),
                        fs::read(&path).unwrap(),
                    ),
                );
            }
        }
    }
    out
}

fn json_of(out: &Output) -> Value {
    assert!(out.status.success(), "{}", stderr(out));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn stacked_cli_read_commands_merge_delta_without_writing_a_single_byte_and_refuse_legacy_pointer() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");

    let seed_out = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(seed_out.status.success(), "{}", stderr(&seed_out));
    let state = lane.join(".devmap/codeintel");
    let delta_path = state.join("delta.sqlite");
    assert!(!delta_path.exists(), "a clean lane has no delta yet");

    // A caller of `beta` that only the delta holds, synced by the explicit write verb.
    fs::write(
        lane.join("src/a.py"),
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n\n\ndef stacked_delta_fn():\n    return beta()\n",
    )
    .unwrap();
    let sync_out = fx.devmap(&lane, &["--json", "seed", "--sync-delta"]);
    assert!(sync_out.status.success(), "{}", stderr(&sync_out));
    assert!(delta_path.is_file());

    // An edit that is not synced: a read must not ingest it.
    fs::write(
        lane.join("src/c.py"),
        "def unsynced_after_delta():\n    return 2\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let before = state_snapshot(&state);

    let impact = json_of(&fx.devmap(&lane, &["--json", "impact", "--", "beta"]));
    let callers: Vec<&str> = impact["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["source_symbol"].as_str())
        .collect();
    assert!(
        callers.iter().any(|c| c.contains("stacked_delta_fn")),
        "impact over a synced lane must list the delta caller: {impact}"
    );
    let search = json_of(&fx.devmap(&lane, &["--json", "search", "--", "stacked_delta_fn"]));
    assert_eq!(search["shown"], 1, "{search}");
    let unsynced = json_of(&fx.devmap(&lane, &["--json", "search", "--", "unsynced_after_delta"]));
    assert_eq!(
        unsynced["shown"], 0,
        "a read must not sync an edit: {unsynced}"
    );
    json_of(&fx.devmap(&lane, &["--json", "explore", "--", "stacked_delta_fn"]));
    json_of(&fx.devmap(&lane, &["--json", "deps", "--", "src/a.py"]));

    let after = state_snapshot(&state);
    let changed: Vec<&PathBuf> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect();
    assert!(
        changed.is_empty(),
        "CLI read commands changed a byte or an mtime under the lane's state dir: {changed:?}"
    );
    assert!(!state.join("delta.sqlite-wal").exists());
    assert!(!state.join("delta.sqlite-shm").exists());

    // A retired pointer with no store and no lease is refused by name and nothing is created.
    let cold = fx.at("cold");
    let legacy_ref = cold.join(".devmap/codeintel/base_store_ref.json");
    fs::create_dir_all(legacy_ref.parent().unwrap()).unwrap();
    fs::write(
        &legacy_ref,
        serde_json::json!({"anchor_repo_root": fx.main()}).to_string(),
    )
    .unwrap();
    let legacy_search = fx.devmap(&cold, &["--json", "search", "--", "alpha"]);
    assert!(!legacy_search.status.success());
    assert!(
        stderr(&legacy_search).contains("seed --stacked --from"),
        "legacy refusal must name the replacement command: {}",
        stderr(&legacy_search)
    );
    assert!(!cold.join(".devmap/codeintel/stack_lease.json").exists());
    assert!(!cold.join(".devmap/codeintel/delta.sqlite").exists());
}

#[test]
fn a_lane_refuses_the_commands_that_read_the_whole_store_instead_of_answering_from_its_base() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    let seed_out = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(seed_out.status.success(), "{}", stderr(&seed_out));

    for args in [
        &["--json", "routes"][..],
        &["--json", "history"][..],
        &["--json", "snapshots"][..],
    ] {
        let out = fx.devmap(&lane, args);
        assert!(!out.status.success(), "{args:?} must refuse on a lane");
        let err = stderr(&out);
        assert!(
            err.contains("uses a stacked delta store") && err.contains("devmap build"),
            "{args:?} must say why and what to run: {err}"
        );
    }
}

/// The `dead` rows' symbol names.
fn dead_names(dead: &Value) -> Vec<String> {
    dead["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a dead answer has an items list: {dead}"))
        .iter()
        .filter_map(|row| row["symbol_name"].as_str().map(str::to_owned))
        .collect()
}

/// Over a lane, `dead` is the base's list with the delta's edits applied. It
/// cannot see two things the delta does: a base symbol whose only caller the
/// delta deleted is dead and is not listed, and a symbol the delta adds with no
/// caller is dead and is not listed. Either the answer lists them or it says,
/// in the field the engine already uses for "the producer did not see
/// everything", that the delta may have made more symbols dead. A silent,
/// complete-looking list is the failure.
#[test]
fn dead_over_a_lane_never_silently_omits_what_the_delta_made_dead() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");

    let base_dead = json_of(&fx.devmap(&fx.main(), &["--json", "dead"]));
    assert!(
        !dead_names(&base_dead).iter().any(|n| n.contains("alpha")),
        "the fixture's alpha has a caller in the base: {base_dead}"
    );

    let seed_out = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(seed_out.status.success(), "{}", stderr(&seed_out));
    // (a) b.py held the only caller of alpha; the lane deletes it.
    fs::remove_file(lane.join("src/b.py")).unwrap();
    // (b) a symbol the delta adds, with no caller.
    fs::write(
        lane.join("src/c.py"),
        "def orphan_added_by_the_delta():\n    return 3\n",
    )
    .unwrap();
    let sync_out = fx.devmap(&lane, &["--json", "seed", "--sync-delta"]);
    assert!(sync_out.status.success(), "{}", stderr(&sync_out));

    let dead = json_of(&fx.devmap(&lane, &["--json", "dead"]));
    let names = dead_names(&dead);
    let marked = dead["walk_incomplete"]
        .as_str()
        .is_some_and(|reason| reason.contains("delta"));
    for (case, name) in [
        ("the delta deleted the only caller", "alpha"),
        ("the delta added a symbol with no caller", "orphan_added_by_the_delta"),
    ] {
        assert!(
            names.iter().any(|n| n.contains(name)) || marked,
            "{case}: `dead` over the lane neither lists {name} nor says the delta may have made \
             more symbols dead: {dead}"
        );
    }
}

/// The sentence `walk_incomplete` carries for an edit the delta has not
/// ingested. The MCP server says the same thing; both are one function.
const UNSYNCED: &str = "edited file(s) are not in the stacked delta yet";

/// A lane answers from a base plus a delta that a detached hook syncs. When the
/// hook lags, every rerouted CLI command must say which edited files its answer
/// does not cover, in `--json` and in the human output, and the commands that
/// do not go through the engine's freshness exit (`literals`, `doc-drift`,
/// `savings`) must also carry the delta's freshness stamp.
#[test]
fn every_rerouted_cli_command_names_the_edits_a_lane_answer_does_not_cover() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    let seed_out = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(seed_out.status.success(), "{}", stderr(&seed_out));

    // One synced edit, so the lane has a delta, then one the hook has not synced.
    fs::write(
        lane.join("src/a.py"),
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n\n\ndef synced_delta_fn():\n    return beta()\n",
    )
    .unwrap();
    let sync_out = fx.devmap(&lane, &["--json", "seed", "--sync-delta"]);
    assert!(sync_out.status.success(), "{}", stderr(&sync_out));
    fs::write(
        lane.join("src/c.py"),
        "def unsynced_after_delta():\n    return 'UNSYNCED_LITERAL'\n",
    )
    .unwrap();

    let commands: [&[&str]; 12] = [
        &["impact", "--", "beta"],
        &["literals", "--", "UNSYNCED_LITERAL"],
        &["doc-drift"],
        &["savings"],
        &["search", "--", "alpha"],
        &["explore", "--", "alpha"],
        &["deps", "--", "src/a.py"],
        &["dead"],
        &["clones"],
        &["neighbors", "--", "beta"],
        &["skeleton", "--", "src/a.py"],
        &["affected", "--", "beta"],
    ];
    for args in commands {
        let mut json_args = vec!["--json"];
        json_args.extend_from_slice(args);
        let answer = json_of(&fx.devmap(&lane, &json_args));
        let walk = answer["walk_incomplete"].as_str().unwrap_or_default();
        assert!(
            walk.contains(UNSYNCED) && walk.contains("src/c.py"),
            "`devmap {args:?}` over a lagging lane must say which edit it does not cover: {answer}"
        );

        let human = fx.devmap(&lane, args);
        assert!(human.status.success(), "{}", stderr(&human));
        let text = String::from_utf8_lossy(&human.stdout);
        assert!(
            text.contains(UNSYNCED) && text.contains("src/c.py"),
            "`devmap {args:?}` human output must print the same warning: {text}"
        );
    }

    for args in [&["literals", "--", "UNSYNCED_LITERAL"][..], &["doc-drift"], &["savings"]] {
        let mut json_args = vec!["--json"];
        json_args.extend_from_slice(args);
        let answer = json_of(&fx.devmap(&lane, &json_args));
        assert!(
            answer["source_freshness"]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("stacked delta")),
            "`devmap {args:?}` must carry the delta's freshness stamp: {answer}"
        );
    }

    // A plain (non-lane) store's answers are untouched.
    let plain = json_of(&fx.devmap(&fx.main(), &["--json", "impact", "--", "beta"]));
    assert!(
        plain["walk_incomplete"]
            .as_str()
            .is_none_or(|reason| !reason.contains("stacked")),
        "{plain}"
    );
}

/// `dead` over a lane lists the base's abandoned cycles from the base's own
/// scan. A delta that deletes the cycle's file makes that list false, and the
/// delta cannot be re-scanned here, so the scan is dropped and the answer says
/// so, naming the delta.
#[test]
fn dead_over_a_lane_does_not_list_a_cycle_the_delta_deleted() {
    let fx = Fixture::new();
    let main = fx.main();
    fs::write(
        main.join("src/cyc.py"),
        "def cyc_one():\n    return cyc_two()\n\n\ndef cyc_two():\n    return cyc_three()\n\n\ndef cyc_three():\n    return cyc_one()\n",
    )
    .unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "cycle"]);
    git(&main, &["worktree", "add", "-q", "--detach", "../lane2"]);
    fx.build_main();
    let base = json_of(&fx.devmap(&main, &["--json", "dead"]));
    assert!(
        base["dead_clusters"].to_string().contains("cyc_one"),
        "the fixture's cycle is an abandoned cluster in the base: {base}"
    );

    let lane = fx.at("lane2");
    let seeded = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(seeded.status.success(), "{}", stderr(&seeded));
    fs::remove_file(lane.join("src/cyc.py")).unwrap();
    let synced = fx.devmap(&lane, &["--json", "seed", "--sync-delta"]);
    assert!(synced.status.success(), "{}", stderr(&synced));

    let dead = json_of(&fx.devmap(&lane, &["--json", "dead"]));
    assert!(
        !dead.to_string().contains("cyc_one"),
        "the deleted cycle is still listed: {dead}"
    );
    assert!(
        dead["dead_clusters_incomplete"]
            .as_str()
            .is_some_and(|reason| reason.contains("delta") && reason.contains("src/cyc.py")),
        "the answer must say the cluster scan was dropped, naming the delta: {dead}"
    );
}

/// A lane's lease claims a generation, a `HEAD` and a donor root. A lease whose
/// claim disagrees with the base it admitted is refused by every command that
/// opens the lane, naming the field, and `status` never reports the lease's
/// generation as the base's.
#[test]
fn a_lease_that_disagrees_with_its_base_is_refused_by_every_lane_command() {
    let fx = Fixture::new();
    fx.build_main();
    let lane = fx.at("lane");
    let seed_out = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(seed_out.status.success(), "{}", stderr(&seed_out));
    let lease_file = lane.join(".devmap/codeintel/stack_lease.json");
    let honest = fs::read(&lease_file).unwrap();
    let honest_json: Value = serde_json::from_slice(&honest).unwrap();
    let forged_generation = honest_json["generation_id"].as_u64().unwrap() + 41;

    let cases = [
        ("generation_id", serde_json::json!(forged_generation)),
        (
            "head_sha",
            serde_json::json!("0123456789abcdef0123456789abcdef01234567"),
        ),
        (
            "source_repo_root",
            serde_json::json!("/nonexistent/some/other/repo"),
        ),
        (
            "source_repo_root",
            serde_json::json!(lane.canonicalize().unwrap().to_string_lossy()),
        ),
    ];
    // Every (case, command) pair is tried and the failures reported together.
    let mut unguarded = Vec::new();
    for (field, forged) in cases {
        let mut tampered = honest_json.clone();
        tampered[field] = forged.clone();
        fs::write(&lease_file, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();

        for args in [
            &["--json", "impact", "alpha"][..],
            &["--json", "search", "--", "alpha"][..],
            &["--json", "seed", "--sync-delta"][..],
        ] {
            let out = fx.devmap(&lane, args);
            if out.status.success() {
                unguarded.push(format!("{field} = {forged}: `{args:?}` answered over a lease that disagrees with its base"));
            } else if !stderr(&out).contains(&format!("lease {field}")) {
                unguarded.push(format!("{field}: `{args:?}` did not name the field: {}", stderr(&out)));
            }
        }

        let status = fx.devmap(&lane, &["--json", "status"]);
        let st: Value = serde_json::from_slice(&status.stdout).unwrap();
        if st["query_ready"] != Value::Bool(false) {
            unguarded.push(format!("{field} = {forged}: status stayed query_ready: {st}"));
        }
        if !st["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("lease {field}"))
        {
            unguarded.push(format!("{field}: status did not name the field: {st}"));
        }
        if st.get("stacked").is_some() {
            unguarded.push(format!(
                "{field}: status reported a lease generation the base does not hold: {st}"
            ));
        }
    }
    assert!(unguarded.is_empty(), "lease claims that were not refused: {unguarded:#?}");
}
