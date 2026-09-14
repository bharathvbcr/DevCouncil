//! The unresolved-call ledger's write path, attacked at its seams.
//!
//! The ledger write was made ~2.3x faster on a cold build, by changes whose
//! whole requirement is that they change *nothing observable*. That is the
//! dangerous shape: a write that is wrong here still succeeds, and the damage
//! is a ledger that quietly disagrees with the resolution it came from.
//!
//! * The two indexes v21 drops — `idx_unresolved_rows_callee` and
//!   `_class` — were the bulk of the cost. Nothing read them, and
//!   `the_ledger_carries_only_the_index_the_prune_uses` is what keeps them
//!   from being re-added by someone who sees a `SCAN` and reaches for an index.
//!
//! * The `reason` text is formatted once and interned instead of being
//!   `format!`-ed three times. The stored bytes must be exactly what
//!   `format!("{:?}", resolution)` produced before, including whatever the
//!   `Debug` derive does to quotes, backslashes and non-ASCII.
//!
//! * The bucket index over fresh identities is built lazily, on the first live
//!   row that reaches the comparison. A cold write never builds it, so the
//!   cold and warm paths now run *different code* to reach the same ledger,
//!   and the tests below drive both.
//!
//! * v22 moved `source_file`, `reason` and `classification` out of the row and
//!   into `paths` and `unresolved_texts`. The row is smaller and the view puts
//!   the text back, so the whole change is invisible to readers — and its one
//!   new way to lose data is a retention prune that retires an interned row the
//!   ledger still points at. `a_prune_never_strands_a_live_ledger_row` drives
//!   that.
//!
//! Multi-row `INSERT` batching was tried here and removed: measured in
//! isolation it was worth nothing (`persist:write` 0.4576 s unbatched against
//! 0.4568 s at 128 rows), because the insert's cost is b-tree page writes and
//! not statement dispatch. The row-count sweep below outlived it — an insert
//! loop should store exactly what it was handed at any length, whatever shape
//! the statement takes.
//!
//! The row counts are deliberately exact. A test that asserts "at least N rows"
//! passes a write that inserted some of them twice.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_resolve::model::{
    Resolution, ResolutionResult, UnresolvedClass, UnresolvedKind, UnresolvedReference,
};
use devmap_store::{GenerationWriteOpts, Store};
use rusqlite::Connection;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// One source file, so the generation has a real extraction to hang rows off.
const SOURCE: (&str, &str) = ("hub.py", "def hub():\n    return 1\n");

/// A ledger row whose every text column is distinct and recoverable from `n`.
///
/// Distinct per row on purpose: a write that binds the wrong parameter slot
/// produces a row whose columns belong to its neighbour, and columns that all
/// say "x" cannot show that.
fn row(n: usize) -> UnresolvedReference {
    UnresolvedReference {
        source_file: SOURCE.0.into(),
        source_symbol: format!("hub.py::sym_{n}"),
        callee_name: format!("callee_{n}"),
        kind: UnresolvedKind::Call,
        resolution: Resolution::Unresolved {
            reason: format!("reason_{n}"),
        },
        class: UnresolvedClass::Unresolved,
        receiver: if n.is_multiple_of(3) {
            None
        } else {
            Some(format!("recv_{n}"))
        },
    }
}

/// Write `unresolved` as one generation and return the store directory.
fn write_ledger(dir: &Path, unresolved: Vec<UnresolvedReference>) -> PathBuf {
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![extract_file(SOURCE.0, SOURCE.1)];
    let resolution = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved,
    };
    let analysis = analyze(&extractions, &resolution);
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    db
}

/// Every live ledger row, in insert order, as comparable tuples.
///
/// Resolved through the joins the view uses, because since v22 the path and the
/// reason are ids into `paths` and `unresolved_texts`. Read from the base table
/// rather than from `generation_unresolved` deliberately: the view is scoped to
/// a generation, and these tests are about what the write *stored*.
///
/// A `JOIN` and not a `LEFT JOIN`: a row whose interned id has no text is a
/// broken ledger, and an inner join makes it vanish from this result and fail
/// the count assertion rather than come back as a plausible `NULL`.
fn stored_rows(db: &Path) -> Vec<(String, String, String, String, Option<String>)> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT p.path, u.source_symbol, u.callee_name, r.text, u.receiver
               FROM unresolved_rows u
               JOIN paths p            ON p.id = u.source_file_id
               JOIN unresolved_texts r ON r.id = u.reason_id
              WHERE u.valid_to IS NULL
              ORDER BY u.unresolved_id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

/// What the write is required to have stored for `row(n)`.
fn expected(n: usize) -> (String, String, String, String, Option<String>) {
    let reference = row(n);
    (
        reference.source_file.clone(),
        reference.source_symbol.clone(),
        reference.callee_name.clone(),
        // The pin that matters: the stored `reason` is the `Debug` of the whole
        // `Resolution`, not the `reason` field inside it. Interning must not
        // have changed which of the two reaches the column.
        format!("{:?}", reference.resolution),
        reference.receiver.clone(),
    )
}

/// A write stores exactly the rows it was handed, at every length.
///
/// The counts are the ones a chunked writer would get wrong — 0, 1, and the
/// values either side of 128 and 256 — kept after the batching that motivated
/// them was removed, because they cost microseconds and they are the lengths
/// any future rewrite of this loop would also have to survive.
#[test]
fn a_write_stores_exactly_the_rows_it_was_given_at_every_length() {
    for count in [0usize, 1, 2, 127, 128, 129, 255, 256, 257, 400] {
        let dir = tmp_dir(&format!("ledger-count-{count}"));
        let db = write_ledger(&dir, (0..count).map(row).collect());
        let stored = stored_rows(&db);

        assert_eq!(
            stored.len(),
            count,
            "a write of {count} ledger rows stored {} of them",
            stored.len()
        );
        for (index, actual) in stored.iter().enumerate() {
            assert_eq!(
                actual,
                &expected(index),
                "row {index} of a {count}-row write does not match what was given \
                 (a swapped bind or a re-inserted row looks exactly like this)"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// The interned text is byte-for-byte the `Debug` output, whatever is in it.
///
/// `Debug` for `String` escapes quotes, backslashes and control characters and
/// leaves most non-ASCII alone. Interning hashes the formatted text and hands
/// back a shared `Rc<str>`, so a reason that round-trips through the intern
/// table must come out identical to one that did not — and two rows that share
/// a reason must not come to share anything *else*.
#[test]
fn interning_preserves_every_reason_byte_and_never_merges_two_rows() {
    let awkward = [
        String::new(),
        "plain".to_string(),
        "with \"quotes\"".to_string(),
        "with \\ backslash".to_string(),
        "with \n newline \t tab".to_string(),
        "unicode ✓ ★ 日本語 café".to_string(),
        "emoji 🔥🙂".to_string(),
        "nul-ish \u{7f} and \u{1}".to_string(),
        "x".repeat(4096),
        // Deliberate duplicates: the intern table must hand the same text to
        // three different rows without collapsing the rows themselves.
        "shared".to_string(),
        "shared".to_string(),
        "shared".to_string(),
    ];
    let dir = tmp_dir("ledger-intern");
    let unresolved: Vec<UnresolvedReference> = awkward
        .iter()
        .enumerate()
        .map(|(n, reason)| UnresolvedReference {
            source_file: SOURCE.0.into(),
            source_symbol: format!("hub.py::sym_{n}"),
            callee_name: format!("callee_{n}"),
            kind: UnresolvedKind::Call,
            resolution: Resolution::Unresolved {
                reason: reason.clone(),
            },
            class: UnresolvedClass::Unresolved,
            receiver: None,
        })
        .collect();
    let db = write_ledger(&dir, unresolved.clone());
    let stored = stored_rows(&db);

    assert_eq!(stored.len(), awkward.len(), "a row went missing");
    for (index, reference) in unresolved.iter().enumerate() {
        assert_eq!(
            stored[index].3,
            format!("{:?}", reference.resolution),
            "reason {index} did not survive interning verbatim"
        );
        assert_eq!(
            stored[index].1, reference.source_symbol,
            "row {index} kept its reason but lost its identity: two rows sharing \
             a reason must not come to share a symbol"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The lazily built index does not change the answer it is an index for.
///
/// A cold write never builds the bucket index — there is no live row to ask
/// about — while the second write over the same store does. Both must reach the
/// same ledger, and the second must reach it by keeping rows rather than
/// rewriting them, which is what the validity range is for.
#[test]
fn a_lazily_built_index_reaches_the_same_ledger_as_an_eager_one() {
    let dir = tmp_dir("ledger-lazy");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![extract_file(SOURCE.0, SOURCE.1)];

    // Cold: no live rows, so the index is never built.
    let first = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: (0..300).map(row).collect(),
    };
    let analysis = analyze(&extractions, &first);
    store
        .save_generation_with_opts(
            &extractions,
            &first,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    let after_cold = stored_rows(&db);
    let cold_ids = live_ids(&db);

    // Warm: 300 live rows, so the index is built on the first of them. Two rows
    // dropped and two added, so the comparison has real work and cannot pass by
    // matching everything.
    let mut second: Vec<UnresolvedReference> = (2..300).map(row).collect();
    second.push(row(1000));
    second.push(row(1001));
    let resolution = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: second.clone(),
    };
    let analysis = analyze(&extractions, &resolution);
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();

    assert_eq!(
        after_cold.len(),
        300,
        "the cold write stored the wrong count"
    );
    let after_warm = stored_rows(&db);
    assert_eq!(
        after_warm.len(),
        second.len(),
        "the warm write left the wrong number of live rows"
    );
    let mut want: Vec<_> = second
        .iter()
        .map(|reference| {
            (
                reference.source_file.clone(),
                reference.source_symbol.clone(),
                reference.callee_name.clone(),
                format!("{:?}", reference.resolution),
                reference.receiver.clone(),
            )
        })
        .collect();
    let mut got = after_warm.clone();
    want.sort();
    got.sort();
    assert_eq!(
        got, want,
        "the warm write's live set is not what was resolved"
    );

    // The 298 rows both writes share must be the *same rows*, not rewritten
    // copies of them. Identity is the surrogate id, which only an insert moves.
    let warm_ids = live_ids(&db);
    let kept = cold_ids.iter().filter(|id| warm_ids.contains(id)).count();
    assert_eq!(
        kept, 298,
        "a validity-range write must keep the rows that did not change; \
         {kept} of 298 survived"
    );
    let _ = fs::remove_dir_all(&dir);
}

fn live_ids(db: &Path) -> Vec<i64> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT unresolved_id FROM unresolved_rows WHERE valid_to IS NULL")
        .unwrap();
    let ids = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<i64>, _>>()
        .unwrap();
    ids
}

/// The ledger carries the prune's index and no others.
///
/// `idx_unresolved_rows_callee` and `idx_unresolved_rows_class` were dropped in
/// v21: nothing read either, and together they cost two b-tree insertions on
/// every one of this repository's 180,679 ledger rows. Pinned by name because
/// the way they come back is someone adding a query, noticing a `SCAN`, and
/// indexing the column without checking whether the scan was the right plan.
#[test]
fn the_ledger_carries_only_the_index_the_prune_uses() {
    let dir = tmp_dir("ledger-indexes");
    let db = write_ledger(&dir, (0..10).map(row).collect());
    let conn = Connection::open(&db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
              WHERE type = 'index' AND tbl_name = 'unresolved_rows'
                AND name NOT LIKE 'sqlite_%'
              ORDER BY name",
        )
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        names,
        vec!["idx_unresolved_rows_closed".to_string()],
        "the ledger should carry exactly the prune's partial index"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Retention must never retire something a live ledger row still points at.
///
/// This is the one new way v22 can lose data. The row no longer carries its
/// path, reason or classification — it carries ids into `paths` and
/// `unresolved_texts` — and both of those tables are garbage-collected by
/// `prune_generations_except_latest`, through anti-joins that list the
/// referencing relations *by hand*. Leave the ledger out of either list and the
/// delete succeeds, the id dangles, and `generation_unresolved` inner-joins the
/// row out of every reader's answer. No error, fewer unresolved calls, and a
/// graph that looks more complete than it is.
///
/// Driven through several generations so the prune actually fires, and with the
/// ledger changing between them so there are closed rows to collect as well as
/// live ones to keep.
///
/// **What this does and does not demonstrate**, established by removing each
/// anti-join term and re-running rather than by reading them:
///
/// * The `unresolved_texts` half is load-bearing and this catches it. Drop
///   `classification_id` from the pool's anti-join and this test fails — the
///   pool collects a text live rows still point at.
/// * The `paths` half is **defensive, not load-bearing today**, and this test
///   cannot show otherwise. Remove `unresolved_rows` from that anti-join and
///   everything still passes, because any file with live ledger rows also has a
///   `generation_file_digests` row in a retained generation, and that term
///   holds the path anyway. The term stays because it makes the list complete
///   in its own right instead of correct via an invariant two subsystems away —
///   the digest write already skips files with no fresh rows, and one more
///   skip there would strand ledger rows with nothing to catch it.
///
/// The foreign keys are the other half of the guard, and the half that does not
/// depend on anyone maintaining a list: with `foreign_keys = ON`, a prune that
/// tried the bad delete fails loudly instead of succeeding quietly.
#[test]
fn a_prune_never_strands_a_live_ledger_row() {
    let dir = tmp_dir("ledger-prune");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![extract_file(SOURCE.0, SOURCE.1)];

    // Four generations, each with a different ledger, so earlier rows close and
    // become collectable while the newest stay live.
    //
    // One row per generation names a file that is *not* extracted, so its
    // `paths` entry has no node, no edge and no payload and the ledger is its
    // only referent. Without that the fixture cannot fail: every other path
    // here is held by `generation_nodes` anyway, so dropping the ledger from
    // the anti-join changes nothing and the test passes while proving nothing.
    // Checked by removing that line and watching this fail.
    for generation in 0..4usize {
        let mut unresolved: Vec<UnresolvedReference> =
            (0..40).map(|n| row(n + generation * 10)).collect();
        let mut ghost = row(9000 + generation);
        ghost.source_file = "ghost.py".into();
        unresolved.push(ghost);
        let resolution = ResolutionResult {
            edges: Vec::new(),
            receiver_types: Default::default(),
            reexport_chains: Default::default(),
            unresolved,
        };
        let analysis = analyze(&extractions, &resolution);
        store
            .save_generation_with_opts(
                &extractions,
                &resolution,
                &analysis,
                GenerationWriteOpts::default(),
            )
            .unwrap();
    }

    let pruned = store.prune_generations_except_latest(2).unwrap();
    assert!(pruned > 0, "fixture: the prune had generations to retire");

    let conn = Connection::open(&db).unwrap();
    // Every live row must still resolve all three of its ids. A dangling one
    // drops the row from this count without dropping it from the table.
    let live: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM unresolved_rows WHERE valid_to IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let joinable: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM unresolved_rows u
               JOIN paths p            ON p.id = u.source_file_id
               JOIN unresolved_texts r ON r.id = u.reason_id
               JOIN unresolved_texts c ON c.id = u.classification_id
              WHERE u.valid_to IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        joinable,
        live,
        "the prune stranded {} live ledger row(s): their path or interned text \
         was retired while the row still pointed at it",
        live - joinable
    );
    assert!(live > 0, "fixture: the store should still hold live rows");

    // And the pool is actually collected, or v22 has traded a wide table for a
    // table that grows forever.
    let pool: i64 = conn
        .query_row("SELECT COUNT(*) FROM unresolved_texts", [], |r| r.get(0))
        .unwrap();
    let referenced: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
               SELECT reason_id AS id FROM unresolved_rows
               UNION SELECT classification_id FROM unresolved_rows)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        pool,
        referenced,
        "the interning pool holds {} row(s) nothing references; a pool that is \
         never collected grows for the life of the store",
        pool - referenced
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A single row far larger than any row is meant to be.
///
/// Not hypothetical. An `AmbiguousGlobal` site records one ledger row carrying
/// its **whole candidate list**, and the resolver bounds that at
/// `ambiguity_evidence_bytes` = 128 MiB rather than at a row width — so a
/// single `reason` can be megabytes. The interning table holds each distinct
/// text once and the write binds a borrow of it, so a wide reason shared by
/// several rows should cost one copy; this is the test that says so rather
/// than the comment.
///
/// Wide and tiny rows are mixed in one write rather than tested apart, because
/// a writer that assumed uniform row width is exactly what this catches.
#[test]
fn a_write_survives_rows_far_wider_than_a_row_is_expected_to_be() {
    let dir = tmp_dir("ledger-wide");
    let huge = "candidate,".repeat(120_000); // ~1.2 MB of reason text
    let mut unresolved: Vec<UnresolvedReference> = (0..200).map(row).collect();
    // Spread across the sweep rather than clustered, so a writer that handles
    // a wide row only at the start or only at the end is still caught.
    for at in [0usize, 127, 128, 199] {
        unresolved[at].resolution = Resolution::Unresolved {
            reason: format!("{huge}{at}"),
        };
    }
    let db = write_ledger(&dir, unresolved.clone());
    let stored = stored_rows(&db);

    assert_eq!(
        stored.len(),
        200,
        "a wide row cost the write its other rows"
    );
    for (index, reference) in unresolved.iter().enumerate() {
        assert_eq!(
            stored[index].3,
            format!("{:?}", reference.resolution),
            "row {index} did not survive a write containing megabyte-wide rows"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The ledger at a size the rest of this file does not reach.
///
/// 400,000 rows is roughly twice this repository's cold ledger and the order of
/// magnitude the 12,831-file corpus `verify.sh` calibrates against produces. It
/// exists because every bound this write path has — the `u32` intern id, the
/// `Vec<u32>` of per-row ids, the `Vec<bool>` of matches, the bucket chain —
/// is fine at 300 rows by inspection and is only actually exercised here.
///
/// Both writes are driven, not just the cold one: the second is where the
/// bucket index is built, over 400,000 identities, and where `claim_matching_
/// candidate` walks its chains. A quarter of the rows change, so the comparison
/// cannot pass by matching everything or by matching nothing.
///
/// Ignored by default, not because it is slow — it runs in under a second on
/// this machine — but because its cost is the memory it holds: 800,000
/// `UnresolvedReference`s across the two writes, which is not what a routine
/// `cargo test` on a shared runner should be made to find room for.
/// `cargo test -p devmap-store --test ledger_write -- --ignored`.
#[test]
#[ignore = "large-scale ledger stress; run with --ignored"]
fn the_ledger_holds_at_four_hundred_thousand_rows() {
    const ROWS: usize = 400_000;
    let dir = tmp_dir("ledger-scale");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![extract_file(SOURCE.0, SOURCE.1)];

    let cold = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: (0..ROWS).map(row).collect(),
    };
    let analysis = analyze(&extractions, &cold);
    store
        .save_generation_with_opts(
            &extractions,
            &cold,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    assert_eq!(
        live_ids(&db).len(),
        ROWS,
        "the cold write lost or duplicated rows at scale"
    );
    let cold_ids: std::collections::HashSet<i64> = live_ids(&db).into_iter().collect();

    // A quarter of the ledger replaced: 300,000 kept, 100,000 closed, 100,000
    // inserted. The kept rows must keep their ids.
    let warm: Vec<UnresolvedReference> = (0..ROWS)
        .map(|n| if n < ROWS / 4 { row(n + ROWS) } else { row(n) })
        .collect();
    let resolution = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: warm,
    };
    let analysis = analyze(&extractions, &resolution);
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();

    let warm_ids: std::collections::HashSet<i64> = live_ids(&db).into_iter().collect();
    assert_eq!(warm_ids.len(), ROWS, "the warm write left the wrong count");
    let kept = cold_ids.intersection(&warm_ids).count();
    assert_eq!(
        kept,
        ROWS - ROWS / 4,
        "the unchanged three quarters must be the same rows, not rewritten copies"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Every `UnresolvedClass` must reach the store as its own label.
///
/// The write path does not intern `classification` the way it interns `reason`.
/// `class.label()` is a `&'static str` over eight variants, so the slot it maps
/// to is memoised on the identity of that static rather than found by hashing
/// the text 181,163 times. That memo is the only place in the ledger write
/// where a row's stored value is chosen by something other than the row's own
/// bytes, which makes it the only place where two classes could come to share
/// one text without any row going missing, any count changing, or any
/// constraint firing.
///
/// Nothing else exercises it: every other test in this file writes
/// `UnresolvedClass::Unresolved` for every row, so a memo that returned one
/// slot for all eight classes would pass the entire suite. This writes one row
/// per variant, in one generation, and asks each row for its own label back.
///
/// `HostGlobal` and `External` carry payloads that do *not* appear in the
/// label, and they are given distinct payloads here on purpose: a write that
/// stored the payload instead of the label, or that merged the two because
/// their labels were reached through the same memo entry, is the failure this
/// is looking for.
#[test]
fn every_unresolved_class_keeps_its_own_label() {
    let classes = [
        UnresolvedClass::Builtin,
        UnresolvedClass::HostGlobal {
            environment: "web+node".to_string(),
        },
        UnresolvedClass::LocalBinding,
        UnresolvedClass::External {
            module: "requests".to_string(),
        },
        UnresolvedClass::UninferredReceiver,
        UnresolvedClass::NoNamesake,
        UnresolvedClass::ModulePath,
        UnresolvedClass::Unresolved,
    ];

    // The labels themselves must be distinct, or the assertion below is
    // satisfied by a write that genuinely merged two classes.
    let labels: Vec<&'static str> = classes.iter().map(|class| class.label()).collect();
    let distinct: std::collections::BTreeSet<&&str> = labels.iter().collect();
    assert_eq!(
        distinct.len(),
        labels.len(),
        "two UnresolvedClass variants share a label, so this test cannot tell them apart: {labels:?}"
    );

    let dir = tmp_dir("ledger-classes");
    let unresolved: Vec<UnresolvedReference> = classes
        .iter()
        .enumerate()
        .map(|(n, class)| UnresolvedReference {
            source_file: SOURCE.0.into(),
            source_symbol: format!("hub.py::sym_{n}"),
            callee_name: format!("callee_{n}"),
            kind: UnresolvedKind::Call,
            resolution: Resolution::Unresolved {
                reason: format!("reason_{n}"),
            },
            class: class.clone(),
            receiver: None,
        })
        .collect();

    let db = write_ledger(&dir, unresolved);
    let conn = Connection::open(&db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT u.source_symbol, c.text
               FROM unresolved_rows u
               JOIN unresolved_texts c ON c.id = u.classification_id
              ORDER BY u.unresolved_id",
        )
        .unwrap();
    let stored: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let expected: Vec<(String, String)> = labels
        .iter()
        .enumerate()
        .map(|(n, label)| (format!("hub.py::sym_{n}"), (*label).to_string()))
        .collect();
    assert_eq!(
        stored, expected,
        "a row was stored with another class's label"
    );

    // The pool holds one row per distinct classification and no more: eight
    // labels plus the eight distinct reasons above.
    let pool: i64 = conn
        .query_row("SELECT COUNT(*) FROM unresolved_texts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        pool,
        (labels.len() + classes.len()) as i64,
        "the pool must hold each label once and each reason once"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// Every row must keep its own source file.
///
/// The write path does not resolve a path per row. Consecutive rows
/// overwhelmingly share a source file — the resolver emits per file — so the
/// loop remembers the last path and its id and reuses that id when the next row
/// names the same path. With the classification memo, this is one of only two
/// places in the ledger write where a row's stored value is chosen by something
/// other than the row's own bytes.
///
/// Nothing exercised it, and the gap was found by mutation rather than by
/// reading: `row(n)` hands every fixture row the same `source_file`, so every
/// other test in this file writes a single-file ledger. Replacing the memo's
/// guard `seen == path` with `true` — which stores every row under the *first*
/// row's file — left the entire `devmap-store` suite green. A ledger that
/// attributed every unresolved call in a repository to one file would have been
/// reported by nothing.
///
/// The pattern below mixes runs with alternation on purpose. A grouped fixture
/// exercises only the memo's hit path, and an alternating one only its miss
/// path; the ledger a resolver actually emits does both, and a memo is at its
/// most dangerous on the row right after a switch.
#[test]
fn every_row_keeps_its_own_source_file() {
    // Runs (0,1,2), a switch, a run, single rows, and a switch back.
    const PATTERN: [&str; 12] = [
        "alpha.py", "alpha.py", "alpha.py", "beta.py", "gamma.py", "gamma.py", "alpha.py",
        "beta.py", "beta.py", "gamma.py", "alpha.py", "gamma.py",
    ];

    let dir = tmp_dir("ledger-source-files");
    let unresolved: Vec<UnresolvedReference> = PATTERN
        .iter()
        .enumerate()
        .map(|(n, file)| UnresolvedReference {
            source_file: (*file).into(),
            source_symbol: format!("{file}::sym_{n}"),
            callee_name: format!("callee_{n}"),
            kind: UnresolvedKind::Call,
            resolution: Resolution::Unresolved {
                reason: format!("reason_{n}"),
            },
            class: UnresolvedClass::Unresolved,
            receiver: None,
        })
        .collect();

    let db = write_ledger(&dir, unresolved);
    let stored = stored_rows(&db);
    assert_eq!(stored.len(), PATTERN.len(), "a row went missing");

    let stored_files: Vec<&str> = stored.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(
        stored_files,
        PATTERN.to_vec(),
        "a row was stored under another row's source file"
    );

    // The rows must also still be their own rows: a write that got the files
    // right by rebuilding every row from scratch would satisfy the check above
    // while losing the symbol that says which row it is.
    for (n, row) in stored.iter().enumerate() {
        assert_eq!(
            row.1,
            format!("{}::sym_{n}", PATTERN[n]),
            "row {n} kept its file but lost its identity"
        );
    }

    // Three distinct files reached `paths`, not one repeated id. Asserted
    // separately because the projection above reads the path *through* the
    // join, so a single id would print one path twelve times and the sequence
    // comparison would already have caught it -- but only because the pattern
    // is not constant. This says the property directly.
    let conn = Connection::open(&db).unwrap();
    let distinct: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT source_file_id) FROM unresolved_rows WHERE valid_to IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        distinct, 3,
        "the ledger must hold three distinct source files"
    );

    let _ = fs::remove_dir_all(&dir);
}
