//! B3 — a one-file edit must not rewrite the repository's payloads.
//!
//! Measured on this repository before the fix, with a single line appended to a
//! single source file:
//!
//! ```text
//!   store after a cold build                 150.2 MB
//!   store after the one-line edit            302.8 MB
//!   generation_files rows                    3,062
//!   ...of which byte-identical duplicates    1,530
//!   extraction_json bytes                    164.5 MB  (54% of the store)
//! ```
//!
//! One edited file caused **~82 MB of JSON to be read out of SQLite, moved
//! through Rust one row at a time, and written back**. The carry-forward this
//! store has done since B3's first half landed avoids re-*deriving* an
//! unchanged payload but still re-*materialises* it under the new generation
//! id, and `generation_files` rows average 53.7 KB apiece.
//!
//! Schema v17 stores one payload per (file, content) in `file_payloads` and
//! leaves `generation_files` as a view over a 16-byte membership row. After it,
//! the same edit:
//!
//! ```text
//!   store after the one-line edit            217.6 MB   (-28%)
//!   file_payloads rows                       1,533      (one per file)
//!   extraction_json bytes                    82.4 MB    (-50%, the duplicate half exactly)
//! ```
//!
//! **What this test does not assert, because it is not yet true.**
//! `generation_edges` (96,525 rows) and `generation_unresolved` (89,537) are
//! still re-materialised per generation — about 67 MB of the remaining growth.
//! Those rows are not content-addressable per file the way a payload is: an
//! edge's target depends on the whole corpus, so carrying one forward because
//! its *source* file did not change would be wrong. That is B3's second half,
//! recorded with its measured size rather than asserted here, because a red
//! test standing in for unfinished work reports the same thing as a broken one.
//!
//! The bounds below are *ratios* rather than absolute counts, so they stay
//! meaningful as the fixture grows: what a differential write stores must be
//! proportional to what changed, not to what exists.

use std::path::Path;
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn build(root: &Path) {
    let out = Command::new(devmap())
        .args(["build", "."])
        .current_dir(root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Stored {
    generations: i64,
    /// Membership rows — what a generation says it holds.
    file_rows: i64,
    /// Distinct stored payloads. The number that used to track `file_rows`.
    payloads: i64,
    /// Bytes of serialized extraction, which is where B3's cost actually was.
    payload_bytes: i64,
    edges: i64,
}

fn stored(root: &Path) -> Stored {
    let conn = rusqlite::Connection::open(devmap_extract::paths::store_path(&root)).unwrap();
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    Stored {
        generations: one("SELECT COUNT(*) FROM generations"),
        file_rows: one("SELECT COUNT(*) FROM generation_files"),
        payloads: one("SELECT COUNT(*) FROM file_payloads"),
        payload_bytes: one("SELECT COALESCE(SUM(LENGTH(extraction_json)
                                 + LENGTH(parse_outcome_json)
                                 + LENGTH(engine_json)), 0) FROM file_payloads"),
        edges: one("SELECT COUNT(*) FROM generation_edges"),
    }
}

/// A corpus wide enough that "proportional to the repository" and
/// "proportional to what changed" are far apart: 60 files that each call into
/// the next, so the edge count is well above the file count.
fn write_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    for index in 0..60 {
        let next = (index + 1) % 60;
        std::fs::write(
            root.join(format!("src/m{index}.rs")),
            format!(
                "pub fn f{index}_a() -> u32 {{ 1 }}\n\
                 pub fn f{index}_b() -> u32 {{ f{index}_a() }}\n\
                 pub fn f{index}_c() -> u32 {{ f{next}_a() + f{index}_b() }}\n"
            ),
        )
        .unwrap();
    }
}

#[test]
fn a_one_file_edit_stores_one_file_of_payload() {
    let root = std::env::temp_dir().join(format!("devmap-b3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    write_fixture(&root);

    build(&root);
    let cold = stored(&root);
    assert_eq!(cold.generations, 1, "the cold build is generation 1");
    assert!(
        cold.file_rows >= 60 && cold.edges > cold.file_rows,
        "the fixture must be wide enough for the two proportionalities to \
         differ: {} file rows, {} edges",
        cold.file_rows,
        cold.edges
    );
    assert_eq!(
        cold.payloads, cold.file_rows,
        "one payload per file on a cold build, with nothing yet to share"
    );

    // One line, one file.
    let touched = root.join("src/m0.rs");
    let mut source = std::fs::read_to_string(&touched).unwrap();
    source.push_str("pub fn f0_d() -> u32 { 4 }\n");
    std::fs::write(&touched, source).unwrap();

    build(&root);
    let after = stored(&root);
    assert_eq!(
        after.generations, 2,
        "the edit produced a second generation"
    );
    assert_eq!(
        after.file_rows,
        cold.file_rows * 2,
        "the premise: both generations still *hold* every file — the fix is \
         about what is stored, not about what a generation claims"
    );

    // The bound. One file changed, so at most one new payload may be stored;
    // the other 59 are the same bytes and must be shared, not copied. Stated
    // with room for a corpus where one edit legitimately re-extracts a handful
    // of files, and still an order of magnitude under a full re-materialisation.
    let ceiling = |total: i64| std::cmp::max(total / 10, 4);
    let added_payloads = after.payloads - cold.payloads;
    assert!(
        added_payloads <= ceiling(cold.payloads),
        "a one-file edit stored {added_payloads} new payloads against {} in the \
         repository — payloads are being copied per generation, not shared",
        cold.payloads
    );

    let added_bytes = after.payload_bytes - cold.payload_bytes;
    assert!(
        added_bytes <= cold.payload_bytes / 10,
        "a one-file edit stored {added_bytes} new payload bytes against {} \
         already in the repository. This is B3: measured on DevCouncil itself, \
         one edited file moved ~82 MB of JSON out of SQLite and back in",
        cold.payload_bytes
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// The other half of the same fact: sharing a payload must not make two files
/// report one path.
///
/// A payload is a serialized `Extraction` and an `Extraction` carries its own
/// `file_path`, so a content-addressed store that ignored the file would
/// collapse two byte-identical files into one payload and make both report the
/// same path. `file_id` is in the payload identity for exactly this reason —
/// found by the end-to-end symlink test, which builds that case by construction
/// and failed on the first run of the split.
#[test]
fn two_files_with_identical_content_keep_their_own_paths() {
    let root = std::env::temp_dir().join(format!("devmap-b3-twin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let identical = "pub fn shared_shape() -> u32 { 1 }\n";
    std::fs::write(root.join("src/one.rs"), identical).unwrap();
    std::fs::write(root.join("src/two.rs"), identical).unwrap();
    build(&root);

    let conn = rusqlite::Connection::open(devmap_extract::paths::store_path(&root)).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT p.path FROM generation_files f
               JOIN paths p ON p.id = f.file_id
              WHERE f.generation_id = (SELECT max(id) FROM generations)
                AND p.path LIKE 'src/%'
              ORDER BY p.path",
        )
        .unwrap();
    let paths: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        paths,
        vec!["src/one.rs".to_string(), "src/two.rs".to_string()],
        "both files must be present under their own paths"
    );
    // And they must not be sharing a payload, which is what would make the
    // extraction each of them reports carry the other's path.
    let distinct_payloads: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT payload_id) FROM generation_file_rows m
               JOIN paths p ON p.id = m.file_id
              WHERE p.path LIKE 'src/%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        distinct_payloads, 2,
        "byte-identical files still need their own payload; the path is inside it"
    );

    std::fs::remove_dir_all(&root).unwrap();
}
