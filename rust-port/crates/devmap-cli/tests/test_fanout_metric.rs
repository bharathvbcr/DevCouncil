//! The ambiguity fan-out metric that `verify.sh` gates on must be derived
//! correctly, and the only way to know that is a corpus whose fan-out can be
//! counted by hand.
//!
//! `tools/fanout.sql` reconstructs the resolver's ambiguous call sites from the
//! persisted store: `Confidence::SPECULATIVE` (0.2) has one producer — the
//! `Resolution::AmbiguousGlobal` rung — which emits one `Calls` edge per
//! candidate, so the edges of one call site share (source file, caller symbol,
//! callee name). Nothing in the schema records the callee name, so the SQL
//! recovers it, and a grouping key that is merely plausible would silently
//! produce a number no gate could trust. This pins it against arithmetic done
//! on paper.
//!
//! The test reads the very file `verify.sh` and `tools/memory_model_probe.sh`
//! read. A copied query would prove only that the copy is self-consistent.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Hand-computed fan-out of the fixture below.
///
/// defs/d0.rs, d1.rs, d2.rs each declare `alpha`  -> 3 candidates
/// defs/d0.rs, d1.rs        each declare `beta`   -> 2 candidates
/// defs/d3.rs                     declares `gamma`-> 1 candidate (not ambiguous)
///
/// callers/c0.rs::one   calls alpha, beta   -> groups of 3 and 2
/// callers/c0.rs::two   calls alpha         -> group of 3
/// callers/c0.rs::four  calls alpha TWICE   -> ONE group of 3 (see below)
/// callers/c1.rs::three calls beta          -> group of 2
/// callers/c1.rs::five  calls gamma         -> no group; resolves uniquely
const EXPECTED_SITES: i64 = 5;
const EXPECTED_EDGES: i64 = 3 + 2 + 3 + 3 + 2;
const EXPECTED_SUM_N2: i64 = 9 + 4 + 9 + 9 + 4;
const EXPECTED_MAX_N: i64 = 3;

fn fanout_sql_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/fanout.sql")
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A corpus whose ambiguity is fixed by construction: N same-named `pub fn`
/// declarations in N files, called by name from a file that declares none of
/// them, so the resolution ladder can only reach the `AmbiguousGlobal` rung.
fn write_fixture(root: &Path) {
    write(
        root,
        "defs/d0.rs",
        "pub fn alpha() -> u32 { 0 }\npub fn beta() -> u32 { 1 }\n",
    );
    write(
        root,
        "defs/d1.rs",
        "pub fn alpha() -> u32 { 0 }\npub fn beta() -> u32 { 1 }\n",
    );
    write(root, "defs/d2.rs", "pub fn alpha() -> u32 { 0 }\n");
    write(root, "defs/d3.rs", "pub fn gamma() -> u32 { 2 }\n");
    write(
        root,
        "callers/c0.rs",
        "pub fn one() -> u32 {\n    let _ = alpha();\n    let _ = beta();\n    0\n}\n\
         pub fn two() -> u32 {\n    let _ = alpha();\n    0\n}\n\
         pub fn four() -> u32 {\n    let _ = alpha();\n    let _ = alpha();\n    0\n}\n",
    );
    write(
        root,
        "callers/c1.rs",
        "pub fn three() -> u32 {\n    let _ = beta();\n    0\n}\n\
         pub fn five() -> u32 {\n    let _ = gamma();\n    0\n}\n",
    );
}

struct Fanout {
    files: i64,
    edges: i64,
    sum_n2: i64,
    sites: i64,
    max_n: i64,
    unjoined: i64,
    alt_sum_n2: i64,
    alt_sites: i64,
}

/// Runs `tools/fanout.sql` exactly as the shell gates do: every statement ahead
/// of the `@@RESULT@@` marker is setup, the tail is the single row-producing
/// query.
fn fanout(db: &Path) -> Fanout {
    const MARKER: &str = "-- @@RESULT@@";
    let sql = std::fs::read_to_string(fanout_sql_path()).expect("tools/fanout.sql is readable");
    assert_eq!(
        sql.matches(MARKER).count(),
        1,
        "tools/fanout.sql must carry exactly one {MARKER} line; the split that separates \
         setup from the result query is ambiguous otherwise"
    );
    let (setup, result) = sql
        .split_once(MARKER)
        .expect("tools/fanout.sql carries the result marker");
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch(setup).expect("fanout.sql setup");
    let row: String = conn
        .query_row(result, [], |r| r.get(0))
        .expect("fanout.sql result");
    let parts: Vec<i64> = row
        .split('|')
        .map(|f| f.parse().expect("fanout.sql emits integers"))
        .collect();
    assert_eq!(parts.len(), 8, "fanout.sql emits 8 pipe-separated fields");
    Fanout {
        files: parts[0],
        edges: parts[1],
        sum_n2: parts[2],
        sites: parts[3],
        max_n: parts[4],
        unjoined: parts[5],
        alt_sum_n2: parts[6],
        alt_sites: parts[7],
    }
}

#[test]
fn the_fanout_metric_matches_a_hand_counted_corpus() {
    let root = std::env::temp_dir().join(format!("devmap-fanout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    write_fixture(&root);

    let db = root.join("index.sqlite");
    let out = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--db", db.to_str().unwrap(), "--progress", "never", "build"])
        .arg(&root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let f = fanout(&db);

    // Fixture precondition. If the resolver ever stopped producing the
    // ambiguous fan-out at all, every assertion below would pass vacuously on
    // zeroes, and the gate built on this number would be measuring nothing.
    assert!(
        f.sites > 0 && f.edges > 0,
        "the fixture must actually produce an ambiguous fan-out"
    );
    assert_eq!(f.files, 6, "fixture file count");

    assert_eq!(f.sites, EXPECTED_SITES, "ambiguous call-site groups");
    assert_eq!(f.edges, EXPECTED_EDGES, "fan-out edges");
    assert_eq!(f.sum_n2, EXPECTED_SUM_N2, "sum of N^2 over call sites");
    assert_eq!(f.max_n, EXPECTED_MAX_N, "widest fan-out");

    // Fail-closed contract the shell gates rely on: every fan-out edge must
    // join to the node it points at, and the two independent recoveries of the
    // callee name must agree. Either failing means the grouping key no longer
    // describes the store, and a caller that ignored these would be reporting a
    // number it had not validated.
    assert_eq!(f.unjoined, 0, "every fan-out edge joins to a real node");
    assert_eq!(
        f.alt_sum_n2, f.sum_n2,
        "node-join and name-suffix derivations of sum(N^2) must agree"
    );
    assert_eq!(
        f.alt_sites, f.sites,
        "node-join and name-suffix derivations of the site count must agree"
    );

    // `gamma` has exactly one declaration, so it resolves on the UniqueGlobal
    // rung and must contribute nothing. Without this the metric could be
    // counting all calls rather than ambiguous ones and still match the totals
    // above by coincidence.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let gamma_speculative: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_edges
              WHERE generation_id = (SELECT max(id) FROM generations)
                AND confidence = 0.2 AND target_symbol LIKE '%gamma'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        gamma_speculative, 0,
        "an unambiguous callee must not appear in the fan-out"
    );

    // The fan-out edge count must account for *every* speculative edge in the
    // store. If some other rung starts emitting 0.2 the grouping key stops
    // describing a call site, and this is what notices.
    let speculative_total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_edges
              WHERE generation_id = (SELECT max(id) FROM generations) AND confidence = 0.2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        speculative_total, f.edges,
        "every speculative edge must be accounted for by the fan-out grouping"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// The metric is a *lower bound* on the resolver's own Sum(N^2), and that is a
/// deliberate, documented property rather than an accident.
///
/// `four` calls `alpha` twice. The resolver emits 3 edges per occurrence, then
/// sorts and dedups, so the store keeps 3 — not 6. Anyone reading `sum_n2` as
/// the resolver-side quantity would be over-crediting the denominator of a
/// bytes-per-pair gate, which is the direction that hides a regression. This
/// pins the collapse so the SQL's header comment cannot quietly become false.
#[test]
fn repeated_calls_to_one_ambiguous_name_collapse_into_a_single_site() {
    let root = std::env::temp_dir().join(format!("devmap-fanout-dedup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    write(
        root.as_path(),
        "defs/d0.rs",
        "pub fn alpha() -> u32 { 0 }\n",
    );
    write(
        root.as_path(),
        "defs/d1.rs",
        "pub fn alpha() -> u32 { 0 }\n",
    );
    write(
        root.as_path(),
        "callers/c0.rs",
        "pub fn once_only() -> u32 {\n    let _ = alpha();\n    0\n}\n",
    );
    write(
        root.as_path(),
        "callers/c1.rs",
        "pub fn thrice() -> u32 {\n    let _ = alpha();\n    let _ = alpha();\n    let _ = alpha();\n    0\n}\n",
    );

    let db = root.join("index.sqlite");
    let out = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--db", db.to_str().unwrap(), "--progress", "never", "build"])
        .arg(&root)
        .output()
        .expect("devmap build");
    assert!(out.status.success());

    let f = fanout(&db);
    assert_eq!(f.sites, 2, "one site per (caller, callee), not per call");
    assert_eq!(
        f.edges, 4,
        "2 candidates x 2 callers, occurrences collapsed"
    );
    assert_eq!(
        f.sum_n2, 8,
        "sum(N^2) counts each (caller, callee) once: 2^2 + 2^2, not 2^2 + 3*2^2"
    );
    assert_eq!(f.unjoined, 0);
    assert_eq!(f.alt_sum_n2, f.sum_n2);

    std::fs::remove_dir_all(&root).unwrap();
}
