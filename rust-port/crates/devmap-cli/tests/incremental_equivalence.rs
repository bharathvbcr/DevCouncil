use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn build(root: &std::path::Path) {
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

/// Every edge in the committed generation, as a comparable set.
fn graph(root: &std::path::Path) -> Vec<String> {
    let store = devmap_store::Store::open(root.join(".devcouncil/codeintel/index.sqlite")).unwrap();
    let mut rows = store.latest_edges_for_test().unwrap();
    rows.sort();
    rows
}

/// The committed generation's analysis, as a comparable string.
///
/// Serialized rather than compared field by field so that a field added later
/// is covered without anyone remembering to add it here — the failure this
/// whole test guards against is a part of the generation nobody thought to
/// compare.
fn analysis(root: &std::path::Path) -> String {
    let store = devmap_store::Store::open(root.join(".devcouncil/codeintel/index.sqlite")).unwrap();
    let summary = store
        .latest_analysis()
        .unwrap()
        .expect("a committed generation carries its analysis");
    serde_json::to_string_pretty(&summary).unwrap()
}

/// The dead-code rows `devmap dead` answers from.
///
/// A separate table from the analysis JSON above, and the one a consumer
/// actually reads, so it is compared separately rather than assumed to agree.
fn dead(root: &std::path::Path) -> Vec<String> {
    let store = devmap_store::Store::open(root.join(".devcouncil/codeintel/index.sqlite")).unwrap();
    let mut rows: Vec<String> = store
        .latest_dead_symbols()
        .unwrap()
        .into_iter()
        .map(|report| {
            format!(
                "{}::{} {:.3} exempt={} {:?}",
                report.file_path,
                report.symbol_name,
                report.confidence,
                report.is_exempt,
                report.exemption_reason
            )
        })
        .collect();
    rows.sort();
    rows
}

/// An incremental build must produce the same graph as a cold build.
///
/// B3/SC2 resolves only the files a change can reach: the changed files plus
/// every file mentioning a name whose definition moved. That is sound because
/// the resolver's two global indexes are keyed by symbol name — but "sound
/// because of an argument" is exactly what SC16 was, where an incremental build
/// diverged permanently and no gate could see it. So the property is asserted
/// directly: edit a file, build incrementally, then build the identical tree
/// cold, and require the graphs to be equal.
#[test]
fn an_incremental_build_equals_a_cold_build() {
    let root = std::env::temp_dir().join(format!("devmap-incr-{}", std::process::id()));
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.py"), "def a():\n    return helper()\n").unwrap();
    std::fs::write(src.join("b.py"), "def helper():\n    return 1\n").unwrap();
    std::fs::write(
        src.join("c.py"),
        "from b import helper\n\ndef c():\n    return helper()\n",
    )
    .unwrap();

    build(&root);

    // Change a definition other files depend on: `helper` gains a sibling, so
    // the symbol index changes for a name two other files call.
    std::fs::write(
        src.join("b.py"),
        "def helper():\n    return 1\n\ndef helper_two():\n    return 2\n",
    )
    .unwrap();
    build(&root);
    let incremental = graph(&root);

    // Same tree, no history.
    std::fs::remove_dir_all(root.join(".devcouncil")).unwrap();
    build(&root);
    let cold = graph(&root);

    assert_eq!(
        incremental, cold,
        "an incremental build must produce exactly the cold graph"
    );
    assert!(
        !cold.is_empty(),
        "fixture precondition: the graph is non-empty"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The same property as above, over the part of the generation that is *not*
/// the graph, on a tree where the affected set is genuinely a small subset.
///
/// The test above passes a three-file fixture where editing one file makes the
/// other two affected too, so the incremental path barely narrows anything —
/// and it compares only the edges, which carry forward correctly by
/// construction. Both of those were why it stayed green through a defect that
/// committed 433 dead-code candidates where a cold build found 14: the analyser
/// was handed the changed file's edges alone, and liveness and clustering are
/// global questions that no subset of the edges can answer.
///
/// So this fixture is twenty independent modules, of which an edit touches one,
/// and the assertion is the whole generation: edges, the analysis summary, and
/// the dead-code rows a consumer reads.
#[test]
fn an_incremental_build_equals_a_cold_build_in_analysis_too() {
    let root = std::env::temp_dir().join(format!("devmap-incr-analysis-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();

    // Twenty modules that do not mention each other, so editing one leaves the
    // other nineteen unaffected — which is the case the subset path narrows to
    // and the case the old fixture could not produce.
    for index in 0..20 {
        std::fs::write(
            src.join(format!("mod_{index}.py")),
            format!(
                "def leaf_{index}():\n    return {index}\n\n\ndef caller_{index}():\n    return leaf_{index}()\n"
            ),
        )
        .unwrap();
    }
    // One hub that calls across files, so the graph is not twenty islands.
    std::fs::write(
        src.join("hub.py"),
        "from mod_0 import caller_0\nfrom mod_1 import caller_1\n\n\ndef hub():\n    return caller_0() + caller_1()\n",
    )
    .unwrap();

    build(&root);
    let cold_first = analysis(&root);
    assert!(
        cold_first.contains("\"total_edges\""),
        "fixture precondition: an analysis was committed"
    );

    // Edit one leaf. Nothing else names `leaf_19`, so the affected set is this
    // file alone and the subset path narrows as far as it ever does.
    std::fs::write(
        src.join("mod_19.py"),
        "def leaf_19():\n    return 19\n\n\ndef caller_19():\n    return leaf_19() + 1\n",
    )
    .unwrap();
    build(&root);

    let incremental_graph = graph(&root);
    let incremental_analysis = analysis(&root);
    let incremental_dead = dead(&root);

    // Same tree, no history.
    std::fs::remove_dir_all(root.join(".devcouncil")).unwrap();
    build(&root);

    assert_eq!(
        incremental_graph,
        graph(&root),
        "an incremental build must store exactly the cold graph"
    );
    assert_eq!(
        incremental_dead,
        dead(&root),
        "an incremental build must store exactly the cold dead-code rows; \
         a symbol reported callerless here is one somebody deletes"
    );
    assert_eq!(
        incremental_analysis,
        analysis(&root),
        "an incremental build must store exactly the cold analysis"
    );

    let _ = std::fs::remove_dir_all(&root);
}
