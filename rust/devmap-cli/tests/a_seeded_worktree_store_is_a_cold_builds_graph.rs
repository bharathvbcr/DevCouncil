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

    assert_eq!(
        state_snapshot(&state),
        before,
        "CLI read commands changed a byte or an mtime under the lane's state dir"
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
