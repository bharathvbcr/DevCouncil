//! A stored analysis that cannot be read must not answer like one that is not there.
//!
//! `Store::dead_page` reads its coverage disclosure with
//! `json_remove(analysis_json, '$.dead_symbols', '$.communities')`, so the two
//! unbounded arrays are dropped inside SQLite instead of crossing into this
//! process — measured on the benchmark corpus at 10,122,764 bytes in the column
//! against 200 that survive the strip.
//!
//! That is only safe while corruption and absence stay distinguishable. If a
//! malformed blob came back as SQL `NULL`, `dead_page` would report
//! `analysis: None`, `dead_symbol_coverage_gap` would render "the analysis
//! summary for this generation could not be read" — which is honest — but the
//! same `None` is what a generation with no analysis produces, and nothing
//! would have failed. This pins the stronger behaviour: the read *errors*.
//!
//! **This is a structural guard, not a red test, and it was checked rather than
//! assumed.** Run against the previous spelling — `SELECT analysis_json`, with
//! serde doing the rejecting — it still passes, because that spelling also
//! errored on a malformed blob. There is no pre-fix state in which it fails.
//! What it pins is the property the `json_remove` read now depends on: a
//! future change that let corruption arrive as SQL `NULL` — `json_extract`
//! behaves that way for a missing path — would turn "could not be read" into
//! "is not there", and this is what would catch it.
//!
//! No parsing frontend is used, so this target builds and runs with
//! `--no-default-features`, which is the shape an embedder links.

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
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Insert one `generations` row carrying `analysis_json` verbatim.
fn store_with_analysis(label: &str, analysis_json: &str) -> (Store, PathBuf) {
    let dir = tmp_dir(label);
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).expect("store opens");
    drop(store);
    {
        let conn = rusqlite::Connection::open(&db_path).expect("raw open");
        conn.execute(
            "INSERT INTO generations (created_at, head_sha, analysis_json) VALUES (1.0, 'abc', ?1)",
            rusqlite::params![analysis_json],
        )
        .expect("insert generation");
    }
    (Store::open(&db_path).expect("store reopens"), db_path)
}

#[test]
fn a_malformed_analysis_blob_is_refused_rather_than_read_as_no_analysis() {
    // Control: a well-formed summary reads back, and the stripped columns do
    // not stop the disclosure fields arriving.
    let (healthy, _keep) = store_with_analysis(
        "analysis-ok",
        r#"{"total_files":3,"total_symbols":9,"total_edges":11,"status":"Ok",
            "unresolved_calls":4,"dead_symbols":[],"communities":[]}"#,
    );
    let page = healthy
        .dead_page(16)
        .expect("a healthy analysis reads")
        .expect("the store holds a generation");
    let analysis = page
        .analysis
        .expect("a well-formed summary must survive the strip");
    assert_eq!(analysis.total_files, 3);
    assert_eq!(
        analysis.unresolved_calls, 4,
        "the disclosure fields must survive json_remove, or the strip took too much"
    );

    // The real case: the column holds bytes that are not JSON.
    let (corrupt, _keep) = store_with_analysis("analysis-corrupt", "{not json at all");
    let outcome = corrupt.dead_page(16);
    assert!(
        outcome.is_err(),
        "a corrupt analysis must be an error, not `analysis: None` — None is what \
         a generation with no analysis produces, and the two must not be the same \
         answer. Got: {:?}",
        outcome.map(|page| page.map(|p| (p.generation, p.analysis.is_some())))
    );
}
