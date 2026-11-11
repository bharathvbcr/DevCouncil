//! What a daemon-held store does with edges an OLDER resolver wrote (ft-c1d8cb8e,
//! card criterion 1). Every step is a direct call (`reconcile_connect_time`,
//! `drain_pending_batch`); nothing waits on a clock or a watcher.
//!
//! The older resolver is staged, not run: the store is built by this tree's
//! daemon, then one edge the resolver of another epoch would have written is
//! added to the latest generation. It is added the way that resolver would have
//! left it: its file's open `digest_rows` row counts it too, so the drain's
//! digest guard (stored digest versus freshly resolved digest), and not only the
//! live-row count, is what has to notice it.
//!
//! What the tests pin:
//!
//! * (a) No edit: the sweep queues nothing and the drain returns before any
//!   resolve, so the stale edge survives. This is a documented gap, not a
//!   verdict: the idle daemon cannot see a resolver mismatch until
//!   `reconcile_connect_time` asks the store whether the generation's resolver
//!   is current. It is handed to the owner of `daemon.rs`.
//! * (b) An unrelated edit, drained: the drain resolves the whole tree and the
//!   store closes every live row the fresh resolution does not match, so the
//!   stale edge is gone. True before and after the stamp.
//! * (c) The same bytes rewritten, drained: over a store stamped with an older
//!   resolver the drain now takes the full path (the resolver stamp is part of
//!   `latest_generation_payload_is_current`, which the drain asks), so the edge
//!   is gone. Over a store stamped current it is the identical-tree restamp,
//!   which trusts the stamp and keeps it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_serve::Daemon;
use devmap_store::Store;

const STALE: &str = "other.py::g>lib.py::helper:Calls:0.90000";

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-daemon-epoch-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

struct Fixture {
    dir: PathBuf,
    root: PathBuf,
    db: PathBuf,
}

impl Fixture {
    fn daemon(&self) -> Daemon {
        Daemon::new(Store::open(&self.db).unwrap(), self.root.clone())
    }

    fn store(&self) -> Store {
        Store::open(&self.db).unwrap()
    }

    fn enqueue(&self, file: &str) {
        self.store()
            .enqueue_pending_paths(&[self.root.join(file).to_string_lossy().into_owned()])
            .unwrap();
    }

    fn has_stale_edge(&self) -> bool {
        self.store()
            .latest_edges_for_test()
            .unwrap()
            .iter()
            .any(|row| row == STALE)
    }

    fn exec(&self, sql: &str) -> usize {
        rusqlite::Connection::open(&self.db)
            .unwrap()
            .execute(sql, [])
            .unwrap()
    }

    /// Overwrite (or remove) the resolver stamp in the latest generation, as an
    /// older binary's store has it.
    fn stamp_resolver(&self, epoch: Option<&str>) {
        let changed = match epoch {
            Some(epoch) => self.exec(&format!(
                "UPDATE generations SET analysis_json =
                   json_set(analysis_json, '$.resolver_epoch', '{epoch}')
                  WHERE id = (SELECT max(id) FROM generations)"
            )),
            None => self.exec(
                "UPDATE generations SET analysis_json =
                   json_remove(analysis_json, '$.resolver_epoch')
                  WHERE id = (SELECT max(id) FROM generations)",
            ),
        };
        assert_eq!(changed, 1);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Four files: `lib.py` defines `helper`, `app.py` calls it, `other.py` and
/// `z.py` do not mention it. One generation is written by the daemon's own
/// drain, then `STALE` (a call from `other.py` into `lib.py::helper` that no
/// resolver of this tree produces) is staged into it, with `other.py`'s open
/// digest row counting it.
fn store_with_a_stale_edge(label: &str) -> Fixture {
    let dir = tmp_dir(label);
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("lib.py"), "def helper():\n    return 1\n").unwrap();
    fs::write(
        root.join("app.py"),
        "from lib import helper\n\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    fs::write(root.join("other.py"), "def g():\n    return 1\n").unwrap();
    fs::write(root.join("z.py"), "def z():\n    return 1\n").unwrap();
    let fixture = Fixture {
        db: dir.join("devmap.sqlite"),
        root,
        dir,
    };

    for file in ["lib.py", "app.py", "other.py", "z.py"] {
        fixture.enqueue(file);
    }
    assert_eq!(fixture.daemon().drain_pending_batch().unwrap(), 4);
    assert!(
        !fixture.has_stale_edge(),
        "fixture precondition: this tree does not itself produce the edge"
    );
    assert!(
        fixture
            .store()
            .latest_generation_payload_is_current()
            .unwrap(),
        "fixture precondition: the daemon's own generation is current"
    );

    let added = fixture.exec(
        "INSERT INTO edge_rows
           (source_file_id, target_file_id, source_symbol, target_symbol,
            edge_kind, confidence, resolution, candidate_total, valid_from, valid_to)
         SELECT (SELECT id FROM paths WHERE path = 'other.py'),
                (SELECT id FROM paths WHERE path = 'lib.py'),
                'other.py::g', 'lib.py::helper', 'Calls', 0.9,
                'UniqueGlobal', NULL, max(id), NULL
         FROM generations",
    );
    assert_eq!(added, 1, "the stale edge was staged");
    let counted = fixture.exec(
        "UPDATE digest_rows SET edge_rows = edge_rows + 1
          WHERE valid_to IS NULL AND file_id = (SELECT id FROM paths WHERE path = 'other.py')",
    );
    assert_eq!(
        counted, 1,
        "other.py's open digest row counts the stale edge"
    );
    assert!(
        fixture.has_stale_edge(),
        "the staged store carries the stale edge"
    );
    fixture
}

fn rewrite_same_bytes(root: &Path, file: &str) {
    let path = root.join(file);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, bytes).unwrap();
}

/// (a) No edit. Documented gap: nothing in the daemon looks at the resolver
/// stamp when there is no pending work, so the stale edge outlives both the
/// connect-time sweep and an empty drain. Pinned so that the follow-up in
/// `daemon.rs` (`reconcile_connect_time` queueing a whole-tree rescan when the
/// resolver is stale) flips this test deliberately.
#[test]
fn a_daemon_with_no_edit_keeps_an_older_resolvers_edge_until_the_idle_follow_up() {
    let fixture = store_with_a_stale_edge("no-edit");
    fixture.stamp_resolver(Some("1"));
    let daemon = fixture.daemon();
    assert_eq!(daemon.reconcile_connect_time().unwrap(), 0);
    assert_eq!(daemon.drain_pending_batch().unwrap(), 0);
    assert!(
        fixture.has_stale_edge(),
        "an idle daemon does not re-resolve: the documented gap"
    );
}

/// (b) An unrelated edit, drained. The measurement the card asks for.
#[test]
fn an_unrelated_edit_drained_over_an_older_resolver_store_drops_the_stale_edge() {
    let fixture = store_with_a_stale_edge("unrelated-edit");
    fixture.stamp_resolver(Some("1"));
    fs::write(fixture.root.join("z.py"), "def z():\n    return 2\n").unwrap();
    fixture.enqueue("z.py");
    assert_eq!(fixture.daemon().drain_pending_batch().unwrap(), 1);
    assert!(
        !fixture.has_stale_edge(),
        "the drain resolves the whole tree and the digest guard closes the edge"
    );
    assert!(fixture
        .store()
        .latest_generation_payload_is_current()
        .unwrap());
}

/// (c) The same bytes rewritten, drained, over an older resolver stamp.
#[test]
fn a_same_bytes_rewrite_drained_over_an_older_resolver_store_drops_the_stale_edge() {
    let fixture = store_with_a_stale_edge("same-bytes-older");
    fixture.stamp_resolver(Some("1"));
    rewrite_same_bytes(&fixture.root, "z.py");
    fixture.enqueue("z.py");
    fixture.daemon().drain_pending_batch().unwrap();
    assert!(
        !fixture.has_stale_edge(),
        "a resolver mismatch defeats the identical-tree restamp"
    );
    assert!(
        fixture
            .store()
            .latest_generation_payload_is_current()
            .unwrap(),
        "the drain's generation is stamped by this resolver"
    );
}

/// (c) with no stamp at all: a generation written before the stamp existed.
#[test]
fn a_same_bytes_rewrite_drained_over_an_unstamped_store_drops_the_stale_edge() {
    let fixture = store_with_a_stale_edge("same-bytes-unstamped");
    fixture.stamp_resolver(None);
    rewrite_same_bytes(&fixture.root, "z.py");
    fixture.enqueue("z.py");
    fixture.daemon().drain_pending_batch().unwrap();
    assert!(
        !fixture.has_stale_edge(),
        "an unrecorded resolver is not a current one"
    );
}

/// The paired control: the same rewrite over a CURRENT stamp is the identical-tree
/// restamp, which trusts the stamp and keeps the staged edge. Only the stamp
/// differs between this test and the two above.
#[test]
fn a_same_bytes_rewrite_drained_over_a_current_resolver_store_trusts_the_stamp() {
    let fixture = store_with_a_stale_edge("same-bytes-current");
    rewrite_same_bytes(&fixture.root, "z.py");
    fixture.enqueue("z.py");
    fixture.daemon().drain_pending_batch().unwrap();
    assert!(
        fixture.has_stale_edge(),
        "a current stamp is trusted: the staged edge is a forgery under it"
    );
}
