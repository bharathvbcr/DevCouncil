use devmap_analyze::*;
use devmap_extract::*;
use devmap_resolve::*;
use devmap_store::*;

#[test]
fn test_store_operations() -> anyhow::Result<()> {
    let store = Store::open_in_memory()?;

    // Test Path interning (B10)
    let p1 = store.get_or_create_path_id("src/main.rs")?;
    let p2 = store.get_or_create_path_id("src/main.rs")?;
    assert_eq!(p1, p2);

    // Test Pending path queue (B1)
    store.enqueue_pending_paths(&["src/main.rs".to_string(), "src/lib.rs".to_string()])?;
    let pending = store.get_pending_paths()?;
    assert_eq!(pending.len(), 2);
    assert!(pending.contains(&"src/main.rs".to_string()));
    assert!(pending.contains(&"src/lib.rs".to_string()));

    store.clear_pending_paths(&["src/main.rs".to_string()])?;
    let pending_after = store.get_pending_paths()?;
    assert_eq!(pending_after.len(), 1);

    // Test generation save and FTS repair (R7)
    let ext = extract_file("src/main.rs", "fn main() {}");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);

    let gen_id = store.save_generation(&[ext], &resolution, &analysis)?;
    assert_eq!(gen_id, 1);

    store.repair_fts()?;
    Ok(())
}

#[test]
fn test_deletion_reconciliation_and_incremental_write() -> anyhow::Result<()> {
    let store = Store::open_in_memory()?;

    // Gen 1: f1.py and f2.py
    let ext1 = extract_file("src/f1.py", "def fn1(): pass");
    let ext2 = extract_file("src/f2.py", "def fn2(): pass");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[ext1.clone(), ext2.clone()]);
    let res1 = resolver.resolve_all(&[ext1.clone(), ext2.clone()]);
    let ana1 = analyze(&[ext1.clone(), ext2.clone()], &res1);

    let gen1 = store.save_generation(&[ext1.clone(), ext2.clone()], &res1, &ana1)?;
    assert_eq!(gen1, 1);

    // Gen 2: f2.py is DELETED on disk! Only f1.py remains active!
    let active_paths = vec!["src/f1.py".to_string()];
    let _ = &active_paths;
    let affected_ext = vec![ext1.clone()]; // f1 edited or remaining

    let mut resolver2 = Resolver::new();
    resolver2.index_extractions(std::slice::from_ref(&ext1));
    let res2 = resolver2.resolve_all(std::slice::from_ref(&ext1));
    // Analysed from the resolution this generation writes, not from gen 1's.
    // Reusing `ana1` here described two files while storing one, which the
    // store now refuses: an analysis that does not match the edges beside it is
    // how a generation came to report 433 dead symbols over a graph with 14.
    // The reconciliation this test is about is unaffected — every assertion
    // below is unchanged.
    let ana2 = analyze(std::slice::from_ref(&ext1), &res2);

    let gen2 = store.save_generation_with_opts(
        &affected_ext,
        &res2,
        &ana2,
        GenerationWriteOpts {
            affected_paths: vec!["src/f1.py".to_string()],
            deleted_paths: vec!["src/f2.py".to_string()],
            build_started: None,
            repo_root: None,
            discovery_refusals: None,
        },
    )?;
    assert_eq!(gen2, 2);

    // Verify f2.py is completely absent from gen2 (N2 compliance). Generation
    // numbering alone cannot prove deletion reconciliation.
    let latest_gen = store.latest_generation_id()?.unwrap();
    assert_eq!(latest_gen, 2);
    assert!(
        store.latest_file("src/f2.py")?.is_none(),
        "deleted file leaked into the latest generation"
    );
    assert!(
        store.latest_edges_for_file("src/f2.py", 0.0)?.is_empty(),
        "deleted file retained live edges in the latest generation"
    );

    Ok(())
}

#[test]
fn test_generation_pruning() -> anyhow::Result<()> {
    let store = Store::open_in_memory()?;

    let ext = extract_file("src/f1.py", "def fn1(): pass");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let res = resolver.resolve_all(std::slice::from_ref(&ext));
    let ana = analyze(std::slice::from_ref(&ext), &res);

    for _ in 0..10 {
        store.save_generation(std::slice::from_ref(&ext), &res, &ana)?;
    }

    // Prune keep latest 2
    let pruned = store.prune_generations_except_latest(2)?;
    assert_eq!(pruned, 8);

    Ok(())
}
