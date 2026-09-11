use devmap_analyze::*;
use devmap_extract::*;
use devmap_resolve::*;
use devmap_store::*;
use std::time::Instant;

#[test]
fn test_large_repo_stress() -> anyhow::Result<()> {
    let mut extractions = Vec::new();
    let num_files = 1000;

    println!("Generating synthetic workload of {} files...", num_files);
    let start_gen = Instant::now();

    for i in 0..num_files {
        let path = format!("src/module_{}.py", i);
        let code = format!(
            "def fn_{}_a():\n    pass\n\ndef fn_{}_b():\n    fn_{}_a()\n",
            i, i, i
        );
        extractions.push(extract_file(&path, &code));
    }
    let extraction_elapsed = start_gen.elapsed();
    println!("Extraction completed in {:?}", extraction_elapsed);
    assert!(extraction_elapsed.as_secs_f32() < 2.0);

    let start_res = Instant::now();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let resolution_elapsed = start_res.elapsed();
    println!("Resolution completed in {:?}", resolution_elapsed);
    assert!(resolution_elapsed.as_secs_f32() < 2.0);

    let start_ana = Instant::now();
    let analysis = analyze(&extractions, &resolution);
    let analysis_elapsed = start_ana.elapsed();
    println!("Analysis completed in {:?}", analysis_elapsed);
    assert!(analysis_elapsed.as_secs_f32() < 2.0);
    assert_eq!(analysis.total_files, num_files);
    assert_eq!(analysis.total_symbols, num_files * 3);
    // Per file: one `fn_i_b -> fn_i_a` call, plus one containment edge for each
    // of the two declared functions. Asserted by composition rather than as a
    // single magic total, so a regression names which kind changed.
    let calls = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .count();
    let contains = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Contains)
        .count();
    let references = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::References)
        .count();
    assert_eq!(calls, num_files, "one intra-file call per module");
    assert_eq!(contains, num_files * 2, "two declared functions per module");
    assert_eq!(analysis.total_edges, calls + contains + references);
    assert!(!analysis.communities.is_empty());

    for comm in &analysis.communities {
        assert!(comm.cohesion_score >= 0.0 && comm.cohesion_score <= 1.0);
    }
    let dead: std::collections::BTreeSet<_> = analysis
        .dead_symbols
        .iter()
        .filter(|report| !report.is_exempt)
        .map(|report| report.symbol_name.as_str())
        .collect();
    let expected: std::collections::BTreeSet<_> = (0..num_files)
        .map(|index| format!("fn_{index}_b"))
        .collect();
    assert_eq!(dead, expected.iter().map(String::as_str).collect());

    let start_db = Instant::now();
    let store = Store::open_in_memory()?;
    let gen_id = store.save_generation(&extractions, &resolution, &analysis)?;
    let database_elapsed = start_db.elapsed();
    println!("Database save completed in {:?}", database_elapsed);
    assert!(database_elapsed.as_secs_f32() < 2.0);
    assert_eq!(gen_id, 1);

    Ok(())
}
