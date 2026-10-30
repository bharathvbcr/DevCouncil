//! Attack round `rb` on `lane/resolution-budget` (91cb227f).
//!
//! Two kinds of test live here. The ones named `finding_*` fail on the lane
//! head and each is one break. The rest pass: they pin the boundaries the
//! round attacked and found sound, so a later change that moves one fails.
//! The `follow_up_*` tests are ignored because they fail identically on
//! dbd0193a; they are runnable with `--ignored`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = run(root, args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "devmap {args:?} stdout is not JSON ({error}): {stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-attack-rb-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, name: &str, body: &str) {
    std::fs::write(root.join(name), body).unwrap();
}

fn build(root: &Path, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["--json", "build", "."];
    args.extend_from_slice(extra);
    json(root, &args)
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path:?}: {e}")))
        .unwrap()
}

fn export_maps(root: &Path) {
    let _ = run(
        root,
        &[
            "manifest",
            ".",
            "--output",
            "repo_map.json",
            "--graph-output",
            "code_graph.json",
        ],
    );
}

/// `count` files that each define `leaf`, plus one caller.
fn leaf_files(root: &Path, count: usize) {
    for index in 0..count {
        write(root, &format!("p{index}.py"), "def leaf():\n    pass\n");
    }
    write(root, "caller.py", "def run():\n    leaf()\n");
}

const TIGHT: &[&str] = &["--resolve-ambiguity-evidence-bytes", "1024"];

// ---------------------------------------------------------------------------
// Findings.
// ---------------------------------------------------------------------------

/// A tree for which the visits budget changes nothing: `uniq` resolves either
/// way, and `meth` is a method of a class, which a bare call never reaches, so
/// the unbudgeted build leaves its site unresolved with no candidate.
fn skip_tree(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root, "u.py", "def uniq():\n    pass\n");
    write(
        &root,
        "k.py",
        "class K:\n    def meth(self):\n        pass\n",
    );
    write(&root, "c.py", "def run():\n    uniq()\n    meth()\n");
    root
}

/// The visits budget charges `hits.len()` before the candidate filter, and a
/// name it cannot afford is *skipped*, not looked up. Here every hit would have
/// been filtered out (a method a bare call cannot reach), so the capped graph is
/// byte-for-byte the uncapped graph. Fable's ruling: it is still Partial.
/// Unknown is not nothing: the lookup never ran, so nothing can say it would
/// have found nothing. The visits budget is not pre-filtered, and the wording
/// says "unknown" rather than claiming a loss or a clean bill.
#[test]
fn a_skipped_lookup_that_would_have_lost_nothing_is_still_partial_and_says_unknown() {
    let free = skip_tree("skip-free");
    let capped = skip_tree("skip-capped");
    let uncapped_build = build(&free, &[]);
    let capped_build = build(&capped, &["--resolve-candidate-visits", "1"]);

    // The counts are the uncapped build's: this is the case where the skip
    // happened to cost nothing, and it is reported all the same.
    for (key, value) in uncapped_build.as_object().unwrap() {
        if key == "edges" || key == "symbols" || key.starts_with("unresolved_") {
            assert_eq!(
                value, &capped_build[key],
                "`{key}` differs, so the capped graph is not the uncapped one"
            );
        }
    }

    let gaps = capped_build["resolution_budget"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "the skip is a gap: {capped_build}");
    assert_eq!(gaps[0]["limit"], "candidate visits");
    assert!(
        gaps[0]["names_skipped"].as_u64().unwrap() >= 1,
        "{capped_build}"
    );
    let status = json(&capped, &["--json", "status"]);
    let reason = status["degraded_reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("name lookup(s) skipped under `--resolve-candidate-visits`")
            && reason.contains("what they would have resolved is unknown"),
        "the status names the state: skipped, and unknown: {reason}"
    );
    assert!(
        !reason.contains("nothing lost") && !reason.contains("0 candidate(s) dropped"),
        "unknown is not nothing-lost: {reason}"
    );
    assert_eq!(status["coverage_gaps"]["resolution_budget"]["total"], 1);
    export_maps(&capped);
    assert_eq!(
        read_json(&capped.join("repo_map.json"))["graph_degraded"],
        true
    );
}

/// FINDING 2. One name that has a candidate list per language family loses
/// candidates in every list, but `candidates_dropped` keeps only the widest
/// loss. Forty Python and forty JavaScript `leaf` declarations, each list cut
/// from 40 to 8, drop 64 candidates; the build says 32.
#[test]
fn finding_dropped_candidates_are_summed_over_every_list_of_a_name() {
    let root = scratch("two-lists");
    for index in 0..40 {
        write(&root, &format!("p{index}.py"), "def leaf():\n    pass\n");
        write(
            &root,
            &format!("j{index}.js"),
            "function leaf() {}\nmodule.exports = { leaf };\n",
        );
    }
    write(&root, "caller.py", "def run():\n    leaf()\n");
    write(&root, "caller.js", "function run() {\n  leaf();\n}\n");

    let result = build(&root, TIGHT);
    let gaps = result["resolution_budget"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{result}");
    assert_eq!(gaps[0]["sites"], 2, "both callers carry a capped row");
    assert_eq!(
        gaps[0]["candidates_dropped"], 64,
        "two lists of 40 each cut to 8 drop 64 candidates, not the widest 32: {result}"
    );
}

/// FINDING 3. A build that proves nothing changed re-states the discovery
/// refusals it just re-measured ("discovery refused N file(s) — these are
/// absent from the graph"), so "No source changes" never reads as "complete".
/// It states nothing about a resolution budget: the generation it calls "still
/// current" is capped, and neither the JSON result (no `resolution_budget` key
/// at all, where every other build emits an array) nor the summary says so.
/// It also leaves a capped generation capped when the caller has just raised
/// the budget, with no word that the cap stands.
#[test]
fn finding_an_unchanged_build_restates_the_cap_its_generation_carries() {
    let root = scratch("unchanged-capped");
    leaf_files(&root, 40);
    let first = build(&root, TIGHT);
    assert_eq!(first["resolution_budget"].as_array().unwrap().len(), 1);

    // Nothing changed; the budget is now generous.
    let again = build(&root, &[]);
    assert_eq!(again["unchanged"], true, "{again}");
    let status = json(&root, &["--json", "status"]);
    assert_eq!(
        status["coverage_gaps"]["resolution_budget"]["total"], 1,
        "the generation is still capped: {status}"
    );
    let restated = again["resolution_budget"]
        .as_array()
        .map_or(0, |gaps| gaps.len());
    assert_eq!(
        restated, 1,
        "an unchanged build must say the generation it keeps is capped: {again}"
    );
}

/// FINDING 4. `ResolutionBudgetGap::sample_names` quotes up to ten callee names
/// verbatim, and `describe()` puts them in the one sentence that becomes the
/// stored gap row, the build JSON, `analysis_json`, `repo_map.json`'s
/// `graph_degraded_reason`, `code_graph.json`'s `analysis_status` and the
/// `walk_incomplete` of every query envelope (nine times in one `explore`).
/// Nothing bounds a name: a 100,000-character identifier makes each of those a
/// 100 KB string. Every other reason that quotes the tree goes through
/// `devmap_query::bounded_text`.
#[test]
fn finding_a_budget_gap_bounds_the_names_it_quotes() {
    let root = scratch("long-name");
    let name = "a".repeat(20_000);
    for index in 0..20 {
        write(
            &root,
            &format!("p{index}.py"),
            &format!("def {name}():\n    pass\n"),
        );
    }
    write(&root, "caller.py", &format!("def run():\n    {name}()\n"));
    let result = build(&root, TIGHT);
    let gaps = result["resolution_budget"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "the cap must still be named");

    export_maps(&root);
    let reason = read_json(&root.join("repo_map.json"))["graph_degraded_reason"]
        .as_str()
        .unwrap_or("")
        .to_string();
    // "Bounded" means the reason's own component limit plus its framing, not a
    // particular remedy: a per-name cut and a cut at the sink both pass.
    let limit = devmap_query::MAX_REASON_COMPONENT_BYTES + 1024;
    assert!(
        reason.len() < limit,
        "graph_degraded_reason quotes an unbounded callee name: {} bytes (limit {limit})",
        reason.len()
    );
    let search = json(&root, &["--json", "search", "run"]);
    let walk = search["walk_incomplete"].as_str().unwrap_or("");
    assert!(
        walk.contains("budget exceeded") && walk.len() < limit,
        "a query envelope carries the cap in a bounded sentence: {} bytes (limit {limit})",
        walk.len()
    );
}

// ---------------------------------------------------------------------------
// Boundaries the round attacked and found sound.
// ---------------------------------------------------------------------------

/// A declaration written twice in one file is one target. Forty-one
/// declarations over forty distinct targets drop 32, not 33.
#[test]
fn duplicate_declarations_do_not_inflate_the_dropped_count() {
    let root = scratch("dup-total");
    leaf_files(&root, 40);
    write(
        &root,
        "p0.py",
        "if True:\n    def leaf():\n        pass\nelse:\n    def leaf():\n        pass\n",
    );
    let result = build(&root, TIGHT);
    let gaps = result["resolution_budget"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{result}");
    assert_eq!(gaps[0]["candidates_dropped"], 32, "{result}");
    assert_eq!(gaps[0]["names_truncated"], 1, "{result}");
}

/// Eight distinct targets with one declared twice is nine declarations: the
/// list keeps all eight and drops nothing, so nothing is capped.
#[test]
fn a_list_that_dedups_to_the_prefix_degrades_nothing() {
    let root = scratch("dedup-to-keep");
    leaf_files(&root, 8);
    write(
        &root,
        "p0.py",
        "if True:\n    def leaf():\n        pass\nelse:\n    def leaf():\n        pass\n",
    );
    let result = build(&root, &["--resolve-ambiguity-evidence-bytes", "1"]);
    assert_eq!(result["resolution_budget"].as_array().unwrap().len(), 0);
    export_maps(&root);
    assert_eq!(
        read_json(&root.join("repo_map.json"))["graph_degraded"],
        false
    );
}

/// One past the prefix drops exactly one; `--resolve-degraded-candidates 0`
/// keeps one (never zero), so a list of two drops one.
#[test]
fn one_past_the_prefix_and_a_zero_prefix_drop_exactly_what_they_cut() {
    let root = scratch("one-past");
    leaf_files(&root, 9);
    let result = build(&root, &["--resolve-ambiguity-evidence-bytes", "1"]);
    assert_eq!(
        result["resolution_budget"][0]["candidates_dropped"], 1,
        "{result}"
    );

    let zero = scratch("zero-prefix");
    leaf_files(&zero, 2);
    let result = build(
        &zero,
        &[
            "--resolve-ambiguity-evidence-bytes",
            "1",
            "--resolve-degraded-candidates",
            "0",
        ],
    );
    assert_eq!(
        result["resolution_budget"][0]["candidates_dropped"], 1,
        "{result}"
    );
}

/// A capped build that is also partially parsed carries both reasons on every
/// surface that carries either: `status`, `dead`, `repo_map.json` and
/// `code_graph.json`.
#[test]
fn a_cap_and_a_refused_file_both_reach_every_surface() {
    let root = scratch("two-reasons");
    leaf_files(&root, 40);
    std::fs::write(root.join("bin.py"), [0u8, 1, 255, 254, 0, 0]).unwrap();
    let _ = build(&root, TIGHT);

    let both = |text: &str, surface: &str| {
        assert!(
            text.contains("budget exceeded") && text.contains("refused by discovery"),
            "{surface} lost one of the two reasons: {text}"
        );
    };
    let status = json(&root, &["--json", "status"]);
    both(status["degraded_reason"].as_str().unwrap_or(""), "status");
    let dead = json(&root, &["--json", "dead"]);
    both(dead["walk_incomplete"].as_str().unwrap_or(""), "dead");
    export_maps(&root);
    let map = read_json(&root.join("repo_map.json"));
    both(
        map["graph_degraded_reason"].as_str().unwrap_or(""),
        "repo_map",
    );
    let graph = read_json(&root.join("code_graph.json"));
    both(
        graph["meta"]["devmap_rust"]["analysis_status"]
            .as_str()
            .unwrap_or(""),
        "code_graph",
    );
}

/// An incremental build that edits only a file the cap does not touch still
/// reports the cap on every surface, because resolution covers the whole tree
/// each time.
#[test]
fn an_incremental_rebuild_of_an_unrelated_file_keeps_the_cap_visible() {
    let root = scratch("incremental-unrelated");
    leaf_files(&root, 40);
    let _ = build(&root, TIGHT);
    write(&root, "z.py", "def unrelated():\n    pass\n");
    let rebuilt = build(&root, TIGHT);
    assert_eq!(rebuilt["resolution_budget"].as_array().unwrap().len(), 1);
    let status = json(&root, &["--json", "status"]);
    assert_eq!(status["coverage_gaps"]["resolution_budget"]["total"], 1);
    assert!(status["degraded_reason"]
        .as_str()
        .unwrap_or("")
        .contains("budget exceeded"));
    export_maps(&root);
    assert_eq!(
        read_json(&root.join("repo_map.json"))["graph_degraded"],
        true
    );
}

/// Two budgets exceeded in one build each get their own gap row, and the
/// rows' `sites` add up to the capped ledger rows the store holds.
#[test]
fn two_budgets_at_once_each_name_their_gap_and_the_sites_add_up() {
    let root = scratch("two-budgets");
    for name in 1..=6 {
        for candidate in 0..40 {
            write(
                &root,
                &format!("m{name}_{candidate}.py"),
                &format!("def name{name}():\n    pass\n"),
            );
        }
    }
    let mut caller = String::new();
    for caller_index in 0..3 {
        caller.push_str(&format!("def caller{caller_index}():\n"));
        for name in 1..=6 {
            caller.push_str(&format!("    name{name}()\n"));
        }
    }
    write(&root, "callers.py", &caller);
    let result = build(
        &root,
        &[
            "--resolve-candidate-visits",
            "100",
            "--resolve-ambiguity-evidence-bytes",
            "1024",
        ],
    );
    let gaps = result["resolution_budget"].as_array().unwrap();
    assert_eq!(gaps.len(), 2, "{result}");
    let sites: u64 = gaps.iter().map(|gap| gap["sites"].as_u64().unwrap()).sum();
    assert_eq!(
        sites, 18,
        "6 names x 3 callers, every site capped: {result}"
    );
    let status = json(&root, &["--json", "status"]);
    assert_eq!(status["coverage_gaps"]["resolution_budget"]["total"], 2);
}

// ---------------------------------------------------------------------------
// Follow-ups: the same silence on dbd0193a for any `Partial` status.
// ---------------------------------------------------------------------------

/// `map.html` is rendered from `repo_map.json`, which says `graph_degraded:
/// true`, and prints no word about it. Identical on dbd0193a for a build whose
/// only degradation is a refused file.
#[test]
#[ignore = "FOLLOW-UP, card ft-b13394a0: fails identically on dbd0193a for a Partial status of any reason"]
fn follow_up_map_html_names_a_degraded_graph() {
    let root = scratch("map-html");
    leaf_files(&root, 40);
    let _ = build(&root, TIGHT);
    let manifest = run(&root, &["manifest", "."]);
    assert!(manifest.status.success(), "{manifest:?}");
    assert_eq!(
        read_json(&root.join(".devmap").join("repo_map.json"))["graph_degraded"],
        true
    );
    let out = run(&root, &["map-html", "."]);
    assert!(out.status.success(), "{out:?}");
    let html = std::fs::read_to_string(root.join(".devmap").join("map.html")).unwrap();
    assert!(
        html.to_lowercase().contains("degraded"),
        "map.html renders a degraded graph as a clean one"
    );
}

/// `devmap html` (the graph page) has no access to the analysis status at all.
#[test]
#[ignore = "FOLLOW-UP, card ft-b13394a0: fails identically on dbd0193a for a Partial status of any reason"]
fn follow_up_graph_html_names_a_degraded_graph() {
    let root = scratch("graph-html");
    leaf_files(&root, 40);
    let _ = build(&root, TIGHT);
    let out = run(&root, &["html", "."]);
    assert!(out.status.success(), "{out:?}");
    let html = std::fs::read_to_string(root.join(".devmap").join("graph.html")).unwrap();
    assert!(
        html.to_lowercase().contains("degraded") || html.to_lowercase().contains("partial"),
        "graph.html renders a degraded graph as a clean one"
    );
}

/// `history` rows carry `parse_failed` and `dead_ambiguous` but nothing that
/// says a generation was built under a cap (or a refusal).
#[test]
#[ignore = "FOLLOW-UP, card ft-b13394a0: fails identically on dbd0193a for a Partial status of any reason"]
fn follow_up_history_marks_a_degraded_generation() {
    let root = scratch("history");
    leaf_files(&root, 40);
    let _ = build(&root, TIGHT);
    let history = json(&root, &["--json", "history"]);
    let row = history["history"][0].to_string().to_lowercase();
    assert!(
        row.contains("degraded") || row.contains("partial") || row.contains("capped"),
        "history reports a capped generation as an ordinary one: {row}"
    );
}
