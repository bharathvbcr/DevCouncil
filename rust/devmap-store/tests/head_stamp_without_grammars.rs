//! The HEAD accessors and the restamp refusal, in a build without grammars.
//!
//! Split from `head_stamp.rs`, whose fixture indexes a real git repository by
//! extracting it and so is declared `required-features = ["parse"]`. GitPulse
//! links this store without grammars and reads the HEAD stamp, so the readers
//! and the restamp have to be shown working in that shape too.
//!
//! The generation row is written by SQL because a build without grammars
//! writes no generation, as `devmap-query`'s
//! `a_query_only_build_can_call_a_store_current` does for the same reason.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::Store;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-head-stamp-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A store holding one empty generation stamped with `head`.
fn store_with_one_generation(dir: &std::path::Path, head: &str) -> Store {
    let db = dir.join("devmap.sqlite");
    drop(Store::open(&db).expect("a fresh store must open"));
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
         VALUES (1, 1.0, ?1, NULL, '{\"total_files\":0,\"total_symbols\":0,
                 \"total_edges\":0,\"dead_symbols\":[],\"communities\":[],\"status\":\"Ok\"}')",
        [head],
    )
    .unwrap();
    drop(conn);
    Store::open(&db).unwrap()
}

/// The two head accessors answer identically, and this target is the guard
/// that they are both reachable without the parsing frontend.
///
/// They were separate implementations of one read, and had drifted the way two
/// copies do: `latest_generation_head_sha` carried a stray
/// `#[cfg(feature = "parse")]` — debris on a pure SQL read — so an embedder
/// linking `devmap-store` with `default-features = false` could reach the head
/// through one name and not the other. That is not hypothetical: it is exactly
/// how it surfaced, as `dc-regress-store` failing to compile for GitPulse,
/// which links the store without grammars.
///
/// This target is listed in `FEATURE_OFF_SAFE`, so this test is compiled and
/// run by `cargo test -p devmap-store --no-default-features`. Naming the gated
/// method here is what makes that build fail if the gate returns.
///
/// It used to live in `head_stamp.rs` and claim the same, but its fixture
/// extracted a real repository, so it could only ever compile with grammars.
/// It did compile, because a dev-dependency switched `parse` back on, and that
/// is what made the claim look true.
#[test]
fn both_head_accessors_answer_the_same_and_need_no_grammar() {
    let root = tmp_dir("head-accessors");
    let head = "0123456789abcdef0123456789abcdef01234567";
    let store = store_with_one_generation(&root, head);
    assert_eq!(
        store.latest_generation_head().unwrap(),
        store.latest_generation_head_sha().unwrap(),
        "one read, two names: they must not drift apart again"
    );
    assert_eq!(
        store.latest_generation_head_sha().unwrap().as_deref(),
        Some(head)
    );

    let moved = "89abcdef0123456789abcdef0123456789abcdef";
    store.restamp_latest_head(moved).unwrap();
    assert_eq!(
        store.latest_generation_head().unwrap(),
        store.latest_generation_head_sha().unwrap(),
        "and they still agree after a write"
    );
    assert_eq!(
        store.latest_generation_head().unwrap().as_deref(),
        Some(moved),
        "the restamp is what both of them now answer"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn restamp_without_a_generation_refuses() {
    let store = Store::open_in_memory().unwrap();
    let error = store
        .restamp_latest_head("abc1234")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no generation"),
        "an empty store has nothing to restamp: {error}"
    );
}
