//! Adversarial challenge test suite for Milestone 1 storage batching & latency optimizations:
//! 1. 50-row FTS5 chunked batching across 0, 1, 49, 50, 51, 100, 1025 symbols.
//! 2. 64-row unresolved call chunked batching remainder handling across 0, 63, 64, 65, 128, 200 rows.
//! 3. Incremental parse failure counter correctness vs cold-build ground truth across rapid additions/deletions.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_resolve::model::*;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use rusqlite::{params, Connection};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-adv-stress-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// 1. FTS5 50-Row Chunked Batching Stress Tests
// ---------------------------------------------------------------------------

#[test]
fn test_fts5_chunked_batching_boundary_counts() {
    let counts = [0, 1, 49, 50, 51, 100, 1025];

    for &count in &counts {
        let dir = tmp_dir(&format!("fts-boundary-{count}"));
        let db_path = dir.join("devmap.sqlite");
        let store = Store::open(&db_path).unwrap();

        // Generate source with plenty of functions to truncate to exact count
        let mut source = String::new();
        let target_len = count + 2;
        for i in 0..target_len {
            source.push_str(&format!("def fn_{i}():\n    return {i}\n\n"));
        }

        let file_path = format!("mod_{count}.py");
        let mut extraction = extract_file(&file_path, &source);
        extraction.symbols.truncate(count);

        // Normalize names for predictable assertions
        for (ord, sym) in extraction.symbols.iter_mut().enumerate() {
            sym.name = format!("sym_{count}_{ord}");
            sym.qualified_name = format!("mod_{count}::sym_{count}_{ord}");
        }

        assert_eq!(
            extraction.symbols.len(),
            count,
            "precondition: symbols must match exact count {count}"
        );

        let mut resolver = Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&extraction));
        let resolution = resolver.resolve_all(std::slice::from_ref(&extraction)).unwrap();
        let analysis = analyze(std::slice::from_ref(&extraction), &resolution);

        let gen_id = store
            .save_generation(std::slice::from_ref(&extraction), &resolution, &analysis)
            .unwrap();
        assert_eq!(gen_id, 1);

        // Verify SQLite storage directly
        let conn = Connection::open(&db_path).unwrap();

        // Check payload_nodes count
        let nodes_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM payload_nodes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            nodes_count as usize, count,
            "payload_nodes count mismatch for target count {count}"
        );

        // Check nodes_fts count
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            fts_count as usize, count,
            "nodes_fts count mismatch for target count {count}"
        );

        // Verify rowid bitwise integrity: fts_rowid = (payload_id << 24) | ord_in_file
        let payload_id: i64 = conn
            .query_row("SELECT rowid FROM file_payloads LIMIT 1", [], |row| row.get(0))
            .unwrap();

        if count > 0 {
            let mut stmt = conn
                .prepare("SELECT rowid, name, qualified_name, path FROM nodes_fts ORDER BY rowid ASC")
                .unwrap();
            let mut rows = stmt.query([]).unwrap();

            let mut ord = 0usize;
            while let Some(row) = rows.next().unwrap() {
                let rowid: i64 = row.get(0).unwrap();
                let name: String = row.get(1).unwrap();
                let qname: String = row.get(2).unwrap();
                let path: String = row.get(3).unwrap();

                let expected_rowid = (payload_id << 24) | (ord as i64);
                let expected_name = format!("sym_{count}_{ord}");

                assert_eq!(
                    rowid, expected_rowid,
                    "rowid mismatch at ord {ord} for total count {count}"
                );
                assert_eq!(
                    name, expected_name,
                    "symbol name mismatch at ord {ord} for total count {count}"
                );
                assert!(
                    qname.contains(&expected_name),
                    "qname mismatch at ord {ord} for total count {count}"
                );
                assert_eq!(
                    path, file_path,
                    "path mismatch at ord {ord} for total count {count}"
                );

                ord += 1;
            }
            assert_eq!(
                ord, count,
                "iterated row count mismatch for total count {count}"
            );

            // Test search across boundaries
            // First symbol
            let results_first = store.search_symbols(&format!("sym_{count}_0"), 10).unwrap();
            assert!(
                !results_first.is_empty(),
                "search for first symbol sym_{count}_0 failed"
            );
            assert_eq!(results_first[0].name, format!("sym_{count}_0"));

            // Last symbol
            let last_idx = count - 1;
            let results_last = store.search_symbols(&format!("sym_{count}_{last_idx}"), 10).unwrap();
            assert!(
                !results_last.is_empty(),
                "search for last symbol sym_{count}_{last_idx} failed"
            );
            assert_eq!(results_last[0].name, format!("sym_{count}_{last_idx}"));

            // Intermediate chunk boundaries if applicable
            if count > 50 {
                let mid_49 = store.search_symbols(&format!("sym_{count}_49"), 10).unwrap();
                assert!(!mid_49.is_empty(), "search for sym_{count}_49 failed");
                let mid_50 = store.search_symbols(&format!("sym_{count}_50"), 10).unwrap();
                assert!(!mid_50.is_empty(), "search for sym_{count}_50 failed");
            }
        }
    }
}

#[test]
fn test_fts5_chunked_batching_tokenizer_special_characters() {
    let dir = tmp_dir("fts-tokenizer-special");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    // Create 75 symbols (spanning across 50-chunk boundary) with mixed naming conventions:
    // underscores, camelCase, numbers, prefix/suffix
    let mut source = String::new();
    for i in 0..75 {
        if i % 3 == 0 {
            source.push_str(&format!("def Special_Alpha_Token_{i}(): pass\n"));
        } else if i % 3 == 1 {
            source.push_str(&format!("def camelCaseIdentifier_{i}(): pass\n"));
        } else {
            source.push_str(&format!("def under_score_var_fn_{i}(): pass\n"));
        }
    }

    let mut extraction = extract_file("special.py", &source);
    extraction.symbols.truncate(75);
    assert_eq!(extraction.symbols.len(), 75);

    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&extraction));
    let resolution = resolver.resolve_all(std::slice::from_ref(&extraction)).unwrap();
    let analysis = analyze(std::slice::from_ref(&extraction), &resolution);

    store
        .save_generation(std::slice::from_ref(&extraction), &resolution, &analysis)
        .unwrap();

    // Search across boundary (around index 50)
    // Symbol 49 is i=49 (49 % 3 == 1 => camelCaseIdentifier_49)
    let s49 = store.search_symbols("camelCaseIdentifier_49", 10).unwrap();
    assert_eq!(s49.len(), 1, "failed to find camelCaseIdentifier_49");

    // Symbol 50 is i=50 (50 % 3 == 2 => under_score_var_fn_50)
    let s50 = store.search_symbols("under_score_var_fn_50", 10).unwrap();
    assert_eq!(s50.len(), 1, "failed to find under_score_var_fn_50");

    // Symbol 51 is i=51 (51 % 3 == 0 => Special_Alpha_Token_51)
    let s51 = store.search_symbols("Special_Alpha_Token_51", 10).unwrap();
    assert_eq!(s51.len(), 1, "failed to find Special_Alpha_Token_51");

    // Sub-token searches
    let tokens = store.search_symbols("Alpha", 50).unwrap();
    assert_eq!(tokens.len(), 25, "expected 25 Alpha tokens");
}

// ---------------------------------------------------------------------------
// 2. Unresolved Call 64-Row Chunked Batching Stress Tests
// ---------------------------------------------------------------------------

#[test]
fn test_unresolved_call_chunked_batching_remainders() {
    let test_counts = [0, 63, 64, 65, 128, 200];

    for &count in &test_counts {
        let dir = tmp_dir(&format!("unresolved-batch-{count}"));
        let db_path = dir.join("devmap.sqlite");
        let store = Store::open(&db_path).unwrap();

        let source = "def caller():\n    return 42\n";
        let extraction = extract_file("hub.py", source);

        let mut unresolved = Vec::with_capacity(count);
        for i in 0..count {
            unresolved.push(UnresolvedReference {
                source_file: "hub.py".into(),
                source_symbol: format!("hub.py::caller_{i}"),
                callee_name: format!("missing_fn_{i}"),
                kind: UnresolvedKind::Call,
                resolution: Resolution::Unresolved {
                    reason: "no matching candidate found".into(),
                },
                class: UnresolvedClass::Unresolved,
                receiver: if i % 2 == 0 {
                    Some(format!("receiver_{i}"))
                } else {
                    None
                },
                site: None,
            });
        }

        let resolution = ResolutionResult {
            edges: vec![],
            receiver_types: Default::default(),
            reexport_chains: Default::default(),
            unresolved,
        };
        let analysis = analyze(std::slice::from_ref(&extraction), &resolution);

        let gen = store
            .save_generation(std::slice::from_ref(&extraction), &resolution, &analysis)
            .unwrap();
        assert_eq!(gen, 1);

        let conn = Connection::open(&db_path).unwrap();

        // 1. Verify count in base table unresolved_rows
        let base_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM unresolved_rows WHERE valid_to IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            base_count as usize, count,
            "base table unresolved_rows count mismatch for target count {count}"
        );

        // 2. Verify count in view generation_unresolved
        let view_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM generation_unresolved WHERE generation_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            view_count as usize, count,
            "view generation_unresolved count mismatch for target count {count}"
        );

        // 3. Verify exact content and order
        if count > 0 {
            let mut stmt = conn
                .prepare(
                    "SELECT u.source_symbol, u.callee_name, ur.text, uc.text, u.receiver, u.valid_from, u.valid_to
                     FROM unresolved_rows u
                     JOIN unresolved_texts ur ON ur.id = u.reason_id
                     JOIN unresolved_texts uc ON uc.id = u.classification_id
                     ORDER BY u.rowid ASC",
                )
                .unwrap();
            let mut rows = stmt.query([]).unwrap();

            let mut idx = 0usize;
            let expected_reason = format!("{:?}", Resolution::Unresolved {
                reason: "no matching candidate found".into(),
            });

            while let Some(row) = rows.next().unwrap() {
                let source_sym: String = row.get(0).unwrap();
                let callee: String = row.get(1).unwrap();
                let reason: String = row.get(2).unwrap();
                let class: String = row.get(3).unwrap();
                let receiver: Option<String> = row.get(4).unwrap();
                let valid_from: i64 = row.get(5).unwrap();
                let valid_to: Option<i64> = row.get(6).unwrap();

                assert_eq!(source_sym, format!("hub.py::caller_{idx}"));
                assert_eq!(callee, format!("missing_fn_{idx}"));
                assert_eq!(reason, expected_reason);
                assert_eq!(class, "unresolved");
                if idx.is_multiple_of(2) {
                    assert_eq!(receiver, Some(format!("receiver_{idx}")));
                } else {
                    assert_eq!(receiver, None);
                }
                assert_eq!(valid_from, 1);
                assert_eq!(valid_to, None);

                idx += 1;
            }
            assert_eq!(idx, count);
        }
    }
}

#[test]
fn test_unresolved_call_reconciliation_across_batches() {
    let dir = tmp_dir("unresolved-reconciliation");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    let extraction = extract_file("main.py", "def main(): pass\n");

    // Gen 1: 65 unresolved calls (1 chunk of 64 + 1 remainder)
    let mut unres1 = Vec::with_capacity(65);
    for i in 0..65 {
        unres1.push(UnresolvedReference {
            source_file: "main.py".into(),
            source_symbol: format!("main.py::fn_{i}"),
            callee_name: format!("call_{i}"),
            kind: UnresolvedKind::Call,
            resolution: Resolution::Unresolved { reason: "r1".into() },
            class: UnresolvedClass::Unresolved,
            receiver: None,
            site: None,
        });
    }

    let res1 = ResolutionResult {
        edges: vec![],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: unres1,
    };
    let ana1 = analyze(std::slice::from_ref(&extraction), &res1);
    store
        .save_generation(std::slice::from_ref(&extraction), &res1, &ana1)
        .unwrap();

    // Gen 2: Keep 64 of them, remove index 64, add 2 new (indices 65 and 66)
    // Total in Gen 2: 64 + 2 = 66 unresolved calls
    let mut unres2 = Vec::with_capacity(66);
    for i in 0..64 {
        unres2.push(UnresolvedReference {
            source_file: "main.py".into(),
            source_symbol: format!("main.py::fn_{i}"),
            callee_name: format!("call_{i}"),
            kind: UnresolvedKind::Call,
            resolution: Resolution::Unresolved { reason: "r1".into() },
            class: UnresolvedClass::Unresolved,
            receiver: None,
            site: None,
        });
    }
    for i in 65..=66 {
        unres2.push(UnresolvedReference {
            source_file: "main.py".into(),
            source_symbol: format!("main.py::fn_{i}"),
            callee_name: format!("call_{i}"),
            kind: UnresolvedKind::Call,
            resolution: Resolution::Unresolved { reason: "r1".into() },
            class: UnresolvedClass::Unresolved,
            receiver: None,
            site: None,
        });
    }

    let extraction2 = extract_file("main.py", "def main(): return 1\n");
    let res2 = ResolutionResult {
        edges: vec![],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: unres2,
    };
    let ana2 = analyze(std::slice::from_ref(&extraction2), &res2);
    store
        .save_generation_with_opts(
            std::slice::from_ref(&extraction2),
            &res2,
            &ana2,
            GenerationWriteOpts {
                affected_paths: vec!["main.py".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();

    let conn = Connection::open(&db_path).unwrap();

    // In Gen 2, view generation_unresolved must report exactly 66
    let view_count2: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_unresolved WHERE generation_id = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(view_count2, 66, "Gen 2 must have 66 active unresolved calls");

    // The retired call (fn_64) must have valid_to = 2
    let closed_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM unresolved_rows WHERE valid_to = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(closed_count, 1, "Exactly 1 retired row must have valid_to = 2");

    // The kept 64 calls must have valid_from = 1 and valid_to IS NULL
    let kept_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM unresolved_rows WHERE valid_from = 1 AND valid_to IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept_count, 64, "Exactly 64 kept rows must remain open from gen 1");

    // The fresh 2 calls must have valid_from = 2 and valid_to IS NULL
    let fresh_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM unresolved_rows WHERE valid_from = 2 AND valid_to IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fresh_count, 2, "Exactly 2 fresh rows must be open from gen 2");
}

// ---------------------------------------------------------------------------
// 3. Incremental Parse Failure Counter Differential Oracle Stress Tests
// ---------------------------------------------------------------------------

#[test]
fn test_incremental_parse_failure_counter_lifecycle_oracle() {
    let dir = tmp_dir("parse-failure-lifecycle");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    // In-memory state of files: path -> content
    let mut files: BTreeMap<String, String> = BTreeMap::new();

    // File definitions:
    // .py files: clean python code
    // .ipynb files: valid JSON notebook is clean, malformed JSON is ParseOutcome::Failed
    // .md files: prose documentation (NotApplicable engine)
    let clean_py = "def clean():\n    return 1\n";
    let clean_nb = r#"{"metadata":{"language_info":{"name":"python"}},"cells":[{"cell_type":"code","source":["def clean():\n","    pass\n"]}]}"#;
    let broken_nb = "{malformed json notebook";
    let broken_nb_alt = "{\n  \"cells\": [broken";
    let md_file = "# Prose file with no tree-sitter grammar\n";

    // Step 1: Initial cold build with 10 files
    // 6 clean py, 3 broken ipynb, 1 markdown (prose)
    for i in 0..6 {
        files.insert(format!("clean_{i}.py"), clean_py.to_string());
    }
    for i in 0..3 {
        files.insert(format!("broken_{i}.ipynb"), broken_nb.to_string());
    }
    files.insert("README.md".to_string(), md_file.to_string());

    let assert_parity = |store: &Store,
                         expected_gen: i64,
                         step_desc: &str,
                         current_files: &BTreeMap<String, String>| {
        // Compute canonical ground truth by extracting all current files
        let extractions: Vec<_> = current_files
            .iter()
            .map(|(p, c)| extract_file(p, c))
            .collect();
        let canonical_failures = extractions.iter().filter(|e| e.is_parse_failure()).count() as u64;

        // Query the incremental store's build_history
        let history = store.build_history(expected_gen as usize).unwrap();
        let inc_history = history
            .into_iter()
            .find(|h| h.generation_id == (expected_gen as u32))
            .expect("build history entry must exist");

        // Verify incremental failure count matches canonical
        assert_eq!(
            inc_history.parse_failed, canonical_failures,
            "[{step_desc}] Incremental parse_failed ({}) != canonical ground truth ({}) at gen {expected_gen}",
            inc_history.parse_failed, canonical_failures
        );

        // Run cold build ground truth in a separate store
        let cold_store = Store::open_in_memory().unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let res = resolver.resolve_all(&extractions).unwrap();
        let ana = analyze(&extractions, &res);
        cold_store.save_generation(&extractions, &res, &ana).unwrap();
        let cold_history = cold_store.build_history(1).unwrap().remove(0);

        assert_eq!(
            inc_history.parse_failed, cold_history.parse_failed,
            "[{step_desc}] Incremental parse_failed ({}) != cold store ({}) at gen {expected_gen}",
            inc_history.parse_failed, cold_history.parse_failed
        );
    };

    // Cold build Gen 1
    let extractions1: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions1);
    let res1 = resolver.resolve_all(&extractions1).unwrap();
    let ana1 = analyze(&extractions1, &res1);
    let g1 = store.save_generation(&extractions1, &res1, &ana1).unwrap();
    assert_eq!(g1, 1);
    assert_parity(&store, 1, "Cold build Gen 1 (6 clean py, 3 broken ipynb, 1 md)", &files);

    // Step 2: Add a new broken notebook file (broken_new.ipynb)
    // Failures should increase: 3 -> 4
    files.insert("broken_new.ipynb".to_string(), broken_nb.to_string());
    let fresh_e = vec![extract_file("broken_new.ipynb", &files["broken_new.ipynb"])];
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g2 = store
        .save_generation_with_opts(
            &fresh_e,
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec!["broken_new.ipynb".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g2, 2);
    assert_parity(&store, 2, "Gen 2: broken_new.ipynb added", &files);

    // Step 3: Fix a broken notebook file (broken_0.ipynb becomes clean)
    // Failures should decrease: 4 -> 3
    files.insert("broken_0.ipynb".to_string(), clean_nb.to_string());
    let fresh_e = vec![extract_file("broken_0.ipynb", &files["broken_0.ipynb"])];
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g3 = store
        .save_generation_with_opts(
            &fresh_e,
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec!["broken_0.ipynb".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g3, 3);
    assert_parity(&store, 3, "Gen 3: broken_0.ipynb fixed to clean", &files);

    // Step 4: Delete a broken notebook file (broken_1.ipynb)
    // Failures should decrease: 3 -> 2
    files.remove("broken_1.ipynb");
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g4 = store
        .save_generation_with_opts(
            &[],
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec![],
                deleted_paths: vec!["broken_1.ipynb".into()],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g4, 4);
    assert_parity(&store, 4, "Gen 4: broken_1.ipynb deleted", &files);

    // Step 5: Delete a clean file (clean_1.py)
    // Failures should remain unchanged: 2 -> 2
    files.remove("clean_1.py");
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g5 = store
        .save_generation_with_opts(
            &[],
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec![],
                deleted_paths: vec!["clean_1.py".into()],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g5, 5);
    assert_parity(&store, 5, "Gen 5: clean_1.py deleted", &files);

    // Step 6: Add a new clean file (clean_new.py)
    // Failures should remain unchanged: 2 -> 2
    files.insert("clean_new.py".to_string(), clean_py.to_string());
    let fresh_e = vec![extract_file("clean_new.py", &files["clean_new.py"])];
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g6 = store
        .save_generation_with_opts(
            &fresh_e,
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec!["clean_new.py".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g6, 6);
    assert_parity(&store, 6, "Gen 6: clean_new.py added", &files);

    // Step 7: Edit a broken file to another broken content (broken_2.ipynb -> broken_nb_alt)
    // Failures should remain unchanged: 2 -> 2
    files.insert("broken_2.ipynb".to_string(), broken_nb_alt.to_string());
    let fresh_e = vec![extract_file("broken_2.ipynb", &files["broken_2.ipynb"])];
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g7 = store
        .save_generation_with_opts(
            &fresh_e,
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec!["broken_2.ipynb".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g7, 7);
    assert_parity(&store, 7, "Gen 7: broken_2.ipynb edited with alternate syntax error", &files);

    // Step 8: Edit a clean notebook to become broken (broken_0.ipynb which was clean -> broken_nb)
    // Failures should increase: 2 -> 3
    files.insert("broken_0.ipynb".to_string(), broken_nb.to_string());
    let fresh_e = vec![extract_file("broken_0.ipynb", &files["broken_0.ipynb"])];
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g8 = store
        .save_generation_with_opts(
            &fresh_e,
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec!["broken_0.ipynb".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g8, 8);
    assert_parity(&store, 8, "Gen 8: broken_0.ipynb made broken again", &files);

    // Step 9: Simultaneous flurry:
    // - Fix broken_0.ipynb
    // - Delete broken_2.ipynb
    // - Delete clean_3.py
    // - Add fresh_broken.ipynb
    // - Add fresh_clean.py
    files.insert("broken_0.ipynb".to_string(), clean_nb.to_string());
    files.remove("broken_2.ipynb");
    files.remove("clean_3.py");
    files.insert("fresh_broken.ipynb".to_string(), broken_nb.to_string());
    files.insert("fresh_clean.py".to_string(), clean_py.to_string());

    let fresh_e = vec![
        extract_file("broken_0.ipynb", &files["broken_0.ipynb"]),
        extract_file("fresh_broken.ipynb", &files["fresh_broken.ipynb"]),
        extract_file("fresh_clean.py", &files["fresh_clean.py"]),
    ];
    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g9 = store
        .save_generation_with_opts(
            &fresh_e,
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: vec![
                    "broken_0.ipynb".into(),
                    "fresh_broken.ipynb".into(),
                    "fresh_clean.py".into(),
                ],
                deleted_paths: vec!["broken_2.ipynb".into(), "clean_3.py".into()],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g9, 9);
    assert_parity(&store, 9, "Gen 9: Simultaneous flurry across additions/deletions/edits", &files);

    // Step 10: Eliminate ALL remaining failures
    // Find all files that are parse failures and fix them to clean_nb
    let mut affected_fix = Vec::new();
    let mut fresh_fix = Vec::new();
    for (path, content) in files.iter_mut() {
        let e = extract_file(path, content);
        if e.is_parse_failure() {
            *content = clean_nb.to_string();
            affected_fix.push(path.clone());
            fresh_fix.push(extract_file(path, clean_nb));
        }
    }
    assert!(!affected_fix.is_empty(), "precondition: must have had failures to fix");

    let all_e: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&all_e);
    let res = r.resolve_all(&all_e).unwrap();
    let ana = analyze(&all_e, &res);
    let g10 = store
        .save_generation_with_opts(
            &fresh_fix,
            &res,
            &ana,
            GenerationWriteOpts {
                affected_paths: affected_fix,
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    for (p, c) in &files {
        let e = extract_file(p, c);
        if e.is_parse_failure() {
            eprintln!("FAILING FILE at gen 10: {} (engine: {:?}, outcome: {:?})", p, e.engine, e.parse_outcome);
        }
    }
    assert_eq!(g10, 10);
    assert_parity(&store, 10, "Gen 10: Fix all remaining failures (counter reaches 0)", &files);

    let hist10 = store.build_history(10).unwrap();
    for h in &hist10 {
        eprintln!("gen {} parse_failed {}", h.generation_id, h.parse_failed);
    }
    let gen10_row = hist10.iter().find(|h| h.generation_id == 10).unwrap();
    assert_eq!(
        gen10_row.parse_failed,
        0,
        "gen 10 parse_failed must be exactly 0"
    );
}

// ---------------------------------------------------------------------------
// 4. Angle A: Multi-File FTS5 Chunked Batching with Carry-Forward & Deletion
// ---------------------------------------------------------------------------

#[test]
fn test_fts5_multifile_carry_forward_and_deletion() {
    let dir = tmp_dir("fts-multifile-lifecycle");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    // Gen 1:
    // File A: 51 symbols (spills into single-row statement)
    // File B: 49 symbols (single-row statement only)
    // File C: 100 symbols (exact 2 chunks of 50)
    // Total in Gen 1 = 200 symbols
    let make_ext = |path: &str, count: usize| {
        let mut src = String::new();
        for i in 0..(count + 2) {
            src.push_str(&format!("def fn_{i}(): pass\n"));
        }
        let mut ext = extract_file(path, &src);
        ext.symbols.truncate(count);
        for (ord, sym) in ext.symbols.iter_mut().enumerate() {
            sym.name = format!("{path}_sym_{ord}");
            sym.qualified_name = format!("{path}::{path}_sym_{ord}");
        }
        ext
    };

    let ext_a1 = make_ext("file_a.py", 51);
    let ext_b1 = make_ext("file_b.py", 49);
    let ext_c1 = make_ext("file_c.py", 100);

    let extractions1 = vec![ext_a1, ext_b1, ext_c1];
    let mut r1 = Resolver::new();
    r1.index_extractions(&extractions1);
    let res1 = r1.resolve_all(&extractions1).unwrap();
    let ana1 = analyze(&extractions1, &res1);
    let g1 = store.save_generation(&extractions1, &res1, &ana1).unwrap();
    assert_eq!(g1, 1);

    // Verify Gen 1 FTS count
    assert_eq!(
        store.count_search_symbols("sym").unwrap(),
        200,
        "Gen 1 must have exactly 200 symbols in FTS"
    );

    // Gen 2:
    // File A: edited to 50 symbols (sym_50 removed)
    // File B: carried forward unchanged (49 symbols)
    // File C: deleted (100 symbols removed)
    // File D: added with 1 symbol
    // Expected in Gen 2: 50 + 49 + 1 = 100 symbols!
    let ext_a2 = make_ext("file_a.py", 50);
    let ext_d2 = make_ext("file_d.py", 1);

    let fresh2 = vec![ext_a2, ext_d2];
    let all_ext2 = vec![
        make_ext("file_a.py", 50),
        make_ext("file_b.py", 49),
        make_ext("file_d.py", 1),
    ];
    let mut r2 = Resolver::new();
    r2.index_extractions(&all_ext2);
    let res2 = r2.resolve_all(&all_ext2).unwrap();
    let ana2 = analyze(&all_ext2, &res2);

    let g2 = store
        .save_generation_with_opts(
            &fresh2,
            &res2,
            &ana2,
            GenerationWriteOpts {
                affected_paths: vec!["file_a.py".into(), "file_d.py".into()],
                deleted_paths: vec!["file_c.py".into()],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g2, 2);

    // Verify search in Gen 2:
    // 1. Deleted file C symbols must NOT be searchable
    let c_results = store.search_symbols("file_c.py", 10).unwrap();
    assert!(
        c_results.is_empty(),
        "deleted file C symbols must not appear in search results"
    );

    // 2. Deleted 51st symbol from file A must NOT be searchable
    let a50_res = store.search_symbols("file_a.py_sym_50", 10).unwrap();
    assert!(
        a50_res.is_empty(),
        "removed symbol file_a.py_sym_50 must not appear in search results"
    );

    // 3. Surviving 50 symbols of file A must be searchable
    let a49_res = store.search_symbols("file_a.py_sym_49", 10).unwrap();
    assert_eq!(a49_res.len(), 1);

    // 4. Carried forward file B symbols must be searchable
    let b0_res = store.search_symbols("file_b.py_sym_0", 10).unwrap();
    assert_eq!(b0_res.len(), 1);
    let b48_res = store.search_symbols("file_b.py_sym_48", 10).unwrap();
    assert_eq!(b48_res.len(), 1);

    // 5. New file D symbol must be searchable
    let d0_res = store.search_symbols("file_d.py_sym_0", 10).unwrap();
    assert_eq!(d0_res.len(), 1);

    // 6. Total searchable count in Gen 2
    assert_eq!(
        store.count_search_symbols("sym").unwrap(),
        100,
        "Gen 2 must have exactly 100 symbols in FTS"
    );
}

// ---------------------------------------------------------------------------
// 5. Angle B: Unresolved Statement Cache Churn Across Variable Remainders
// ---------------------------------------------------------------------------

#[test]
fn test_unresolved_statement_cache_churn_across_sizes() {
    let dir = tmp_dir("unresolved-cache-churn");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    let ext = extract_file("root.py", "def root(): pass\n");

    // Sequence of 10 generations with fluctuating remainder sizes
    let sizes = [1, 17, 33, 63, 64, 65, 127, 128, 129, 255];

    for (gen_idx, &size) in sizes.iter().enumerate() {
        let expected_gen = (gen_idx + 1) as u32;
        let mut unresolved = Vec::with_capacity(size);
        for i in 0..size {
            unresolved.push(UnresolvedReference {
                source_file: "root.py".into(),
                source_symbol: format!("root.py::root_{i}"),
                callee_name: format!("callee_{i}"),
                kind: UnresolvedKind::Call,
                resolution: Resolution::Unresolved {
                    reason: format!("reason_for_{size}"),
                },
                class: UnresolvedClass::Unresolved,
                receiver: None,
                site: None,
            });
        }

        let res = ResolutionResult {
            edges: vec![],
            receiver_types: Default::default(),
            reexport_chains: Default::default(),
            unresolved,
        };
        let ana = analyze(std::slice::from_ref(&ext), &res);

        let g = if gen_idx == 0 {
            store.save_generation(std::slice::from_ref(&ext), &res, &ana).unwrap()
        } else {
            store
                .save_generation_with_opts(
                    std::slice::from_ref(&ext),
                    &res,
                    &ana,
                    GenerationWriteOpts {
                        affected_paths: vec!["root.py".into()],
                        deleted_paths: vec![],
                        build_started: None,
                        repo_root: None,
                        discovery_refusals: None,
                        over_limit: None,
                        verify_every_row: false,
                    },
                )
                .unwrap()
        };
        assert_eq!(g, expected_gen);

        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM generation_unresolved WHERE generation_id = ?1",
                params![expected_gen],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count as usize, size,
            "size mismatch at generation {expected_gen}"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Angle C: Fallback Path Stress in `record_parse_failures`
// ---------------------------------------------------------------------------

#[test]
fn test_parse_failure_fallback_when_prev_history_missing() {
    let dir = tmp_dir("parse-failure-fallback");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    let clean_nb = r#"{"metadata":{"language_info":{"name":"python"}},"cells":[{"cell_type":"code","source":["def clean():\n","    pass\n"]}]}"#;
    let broken_nb = "{malformed json notebook";

    // Gen 1: 2 clean notebooks, 2 broken notebooks
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    files.insert("nb1.ipynb".to_string(), clean_nb.to_string());
    files.insert("nb2.ipynb".to_string(), clean_nb.to_string());
    files.insert("broken1.ipynb".to_string(), broken_nb.to_string());
    files.insert("broken2.ipynb".to_string(), broken_nb.to_string());

    let ext1: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r1 = Resolver::new();
    r1.index_extractions(&ext1);
    let res1 = r1.resolve_all(&ext1).unwrap();
    let ana1 = analyze(&ext1, &res1);
    store.save_generation(&ext1, &res1, &ana1).unwrap();

    // Adversarially delete build_history for Gen 1 to force fallback path in Gen 2
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM build_history WHERE generation_id = 1", [])
        .unwrap();
    drop(conn);

    // Gen 2: incremental edit
    // Fix broken1.ipynb, break nb1.ipynb
    // Expected failures = 2 (broken2 and nb1)
    files.insert("broken1.ipynb".to_string(), clean_nb.to_string());
    files.insert("nb1.ipynb".to_string(), broken_nb.to_string());

    let fresh = vec![
        extract_file("broken1.ipynb", &files["broken1.ipynb"]),
        extract_file("nb1.ipynb", &files["nb1.ipynb"]),
    ];
    let all_ext: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r2 = Resolver::new();
    r2.index_extractions(&all_ext);
    let res2 = r2.resolve_all(&all_ext).unwrap();
    let ana2 = analyze(&all_ext, &res2);

    let g2 = store
        .save_generation_with_opts(
            &fresh,
            &res2,
            &ana2,
            GenerationWriteOpts {
                affected_paths: vec!["broken1.ipynb".into(), "nb1.ipynb".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
                over_limit: None,
                verify_every_row: false,
            },
        )
        .unwrap();
    assert_eq!(g2, 2);

    // Verify that the fallback path calculated exactly 2 failures
    let hist2 = store.build_history(5).unwrap();
    let g2_row = hist2.iter().find(|h| h.generation_id == 2).unwrap();
    assert_eq!(
        g2_row.parse_failed, 2,
        "fallback path must accurately count failures as 2"
    );
}

// ---------------------------------------------------------------------------
// 7. Angle D: 30-Generation Differential Fuzz Oracle
// ---------------------------------------------------------------------------

#[test]
fn test_parse_failure_30_generation_fuzz_oracle() {
    let dir = tmp_dir("parse-failure-fuzz");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    let clean_nb = r#"{"metadata":{"language_info":{"name":"python"}},"cells":[{"cell_type":"code","source":["def clean():\n","    pass\n"]}]}"#;
    let broken_nb = "{malformed json notebook";
    let clean_py = "def py_fn(): pass\n";

    // Initial pool of 12 files
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for i in 0..6 {
        files.insert(format!("py_{i}.py"), clean_py.to_string());
    }
    for i in 0..3 {
        files.insert(format!("nb_clean_{i}.ipynb"), clean_nb.to_string());
    }
    for i in 0..3 {
        files.insert(format!("nb_broken_{i}.ipynb"), broken_nb.to_string());
    }

    // Gen 1: Cold start
    let ext1: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
    let mut r = Resolver::new();
    r.index_extractions(&ext1);
    let res = r.resolve_all(&ext1).unwrap();
    let ana = analyze(&ext1, &res);
    store.save_generation(&ext1, &res, &ana).unwrap();

    // Pseudo-random deterministic generator: LCG
    let mut lcg = 123456789u64;
    let mut next_rand = || {
        lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1);
        lcg
    };

    // Run 30 generations of random operations
    for gen in 2..=30 {
        let op_choice = next_rand() % 5;
        let mut affected = Vec::new();
        let mut deleted = Vec::new();
        let mut fresh = Vec::new();

        match op_choice {
            0 => {
                // Break a clean notebook if available
                let clean_candidate = files.iter().find(|(p, c)| p.ends_with(".ipynb") && *c == clean_nb).map(|(p, _)| p.clone());
                if let Some(target) = clean_candidate {
                    files.insert(target.clone(), broken_nb.to_string());
                    fresh.push(extract_file(&target, broken_nb));
                    affected.push(target);
                }
            }
            1 => {
                // Fix a broken notebook if available
                let broken_candidate = files.iter().find(|(p, c)| p.ends_with(".ipynb") && *c == broken_nb).map(|(p, _)| p.clone());
                if let Some(target) = broken_candidate {
                    files.insert(target.clone(), clean_nb.to_string());
                    fresh.push(extract_file(&target, clean_nb));
                    affected.push(target);
                }
            }
            2 => {
                // Add a new file (randomly clean or broken)
                let is_broken = (next_rand() % 2) == 0;
                let new_path = format!("dyn_nb_{gen}.ipynb");
                let new_content = if is_broken { broken_nb } else { clean_nb };
                files.insert(new_path.clone(), new_content.to_string());
                fresh.push(extract_file(&new_path, new_content));
                affected.push(new_path);
            }
            3 => {
                // Delete an existing notebook
                let candidate = files.iter().find(|(p, _)| p.starts_with("dyn_nb_") || p.starts_with("nb_")).map(|(p, _)| p.clone());
                if let Some(target) = candidate {
                    files.remove(&target);
                    deleted.push(target);
                }
            }
            _ => {
                // Edit a clean python file
                let target = "py_0.py".to_string();
                let new_py = format!("def py_fn_{gen}(): return {gen}\n");
                files.insert(target.clone(), new_py.clone());
                fresh.push(extract_file(&target, &new_py));
                affected.push(target);
            }
        }

        if affected.is_empty() && deleted.is_empty() {
            // Touch a file so differential write proceeds
            let target = "py_1.py".to_string();
            let new_py = format!("def py_fn_touch_{gen}(): pass\n");
            files.insert(target.clone(), new_py.clone());
            fresh.push(extract_file(&target, &new_py));
            affected.push(target);
        }

        let all_ext: Vec<_> = files.iter().map(|(p, c)| extract_file(p, c)).collect();
        let mut r = Resolver::new();
        r.index_extractions(&all_ext);
        let res = r.resolve_all(&all_ext).unwrap();
        let ana = analyze(&all_ext, &res);

        let g = store
            .save_generation_with_opts(
                &fresh,
                &res,
                &ana,
                GenerationWriteOpts {
                    affected_paths: affected,
                    deleted_paths: deleted,
                    build_started: None,
                    repo_root: None,
                    discovery_refusals: None,
                    over_limit: None,
                    verify_every_row: false,
                },
            )
            .unwrap();
        assert_eq!(g, gen as u32);

        // Differential Ground Truth Assertion
        let canonical_failures = all_ext.iter().filter(|e| e.is_parse_failure()).count() as u64;
        let hist = store.build_history(50).unwrap();
        let row = hist.iter().find(|h| h.generation_id == (gen as u32)).unwrap();

        assert_eq!(
            row.parse_failed, canonical_failures,
            "[Fuzz Gen {gen}] Incremental failure count ({}) drifted from canonical ground truth ({})",
            row.parse_failed, canonical_failures
        );
    }
}

