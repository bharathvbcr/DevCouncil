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
//!   seed carries them.

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
