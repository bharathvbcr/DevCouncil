//! Q-6: two truncating lists in `repo_map.json` that disclose nothing.
//!
//! `lean_manifest` cuts `important_files` to 15 and `subsystems` to 20, and
//! `consumer_manifest_json` then drops further subsystems whose representative
//! file yields no real directory area. None of it is reported. The keys that
//! mention either list are the lists themselves, so a consumer reading
//! `subsystems` has no way to tell a repository with 20 subsystems from one
//! with 400 — and the agent guides tell agents to navigate by exactly that key.
//!
//! `entry_roots` in the same function already carries `{shown, total,
//! truncated}` for exactly this reason: the pattern exists here, and its two
//! peers were left out of it.
//!
//! Both directions are asserted. A fix that stamps `truncated: true` on every
//! artifact tells a reader nothing they did not already have to assume, so the
//! complete-corpus half below fails against one.

use devmap_analyze::model::{AnalysisStatus, AnalysisSummary, CommunityReport};
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::manifest::generate_manifest_with_edges;
use devmap_query::model::FreshnessInfo;
use serde_json::Value;

fn freshness() -> FreshnessInfo {
    FreshnessInfo::new("abc123".to_string(), 7, 0)
}

fn community(id: u32, members: &[String]) -> CommunityReport {
    CommunityReport {
        community_id: id,
        name: format!("community-{id}"),
        members: members.to_vec(),
        cohesion_score: 0.5,
    }
}

/// `subsystems` fixture: `count` two-file packages, each its own community.
fn packages(count: u32) -> (Vec<Extraction>, Vec<CommunityReport>) {
    let mut extractions = Vec::new();
    let mut communities = Vec::new();
    for index in 0..count {
        let members = vec![
            format!("sub{index:03}/mod_a.py"),
            format!("sub{index:03}/mod_b.py"),
        ];
        for member in &members {
            extractions.push(extract_file(member, "def one():\n    return 1\n"));
        }
        communities.push(community(index, &members));
    }
    (extractions, communities)
}

/// `important_files` fixture: `count` files the manifest treats as important.
fn plans(count: u32) -> Vec<Extraction> {
    (0..count)
        .map(|index| extract_file(&format!("docs/plan{index:03}_PLAN.md"), "# plan\n"))
        .collect()
}

fn render(extractions: &[Extraction], communities: Vec<CommunityReport>) -> Value {
    let analysis = AnalysisSummary {
        // No discovery step ran over this hand-built corpus, so there is no
        // refusal count to report. `None` says that; `0` would claim a walk.
        discovery_refused_files: None,
        total_files: extractions.len(),
        total_symbols: 0,
        total_edges: 0,
        dead_symbols: Vec::new(),
        communities,
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
        // Fields this fixture does not exercise. Spread rather than
        // enumerated so a new analysis field does not break every test
        // literal in the workspace; the one production construction in
        // `analyze()` still names every field exhaustively.
        ..Default::default()
    };
    let (_, json) = generate_manifest_with_edges(extractions, &analysis, freshness(), &[], None);
    serde_json::from_str(&json).expect("the consumer manifest is JSON")
}

fn len_of(map: &Value, key: &str) -> usize {
    map[key].as_array().expect("a list").len()
}

/// The ON direction: 40 communities become 20 entries and 30 important files
/// become 15, and both truncations are now countable by the consumer.
#[test]
fn a_truncated_subsystem_and_important_file_list_carry_their_totals() {
    let (mut extractions, communities) = packages(40);
    extractions.extend(plans(30));
    let map = render(&extractions, communities);

    let subsystems = &map["liveness_meta"]["subsystems"];
    assert_eq!(
        subsystems["shown"], 20,
        "the cap still holds; the point is that it is stated"
    );
    assert_eq!(
        subsystems["shown"].as_u64().unwrap() as usize,
        len_of(&map, "subsystems"),
        "`shown` must count the entries actually emitted, not the entries the \
         cap admitted before later filtering"
    );
    assert_eq!(
        subsystems["total"], 40,
        "`total` is what a complete answer would have held — the count of \
         communities the analysis found"
    );
    assert_eq!(subsystems["truncated"], true);

    let important = &map["liveness_meta"]["important_files"];
    assert_eq!(important["shown"], 15);
    assert_eq!(
        important["shown"].as_u64().unwrap() as usize,
        len_of(&map, "important_files")
    );
    assert_eq!(important["total"], 30);
    assert_eq!(important["truncated"], true);
}

/// The OFF direction: a repository whose lists fit is not reported as cut.
#[test]
fn lists_that_fit_are_not_reported_as_truncated() {
    let (mut extractions, communities) = packages(3);
    extractions.extend(plans(2));
    let map = render(&extractions, communities);

    let subsystems = &map["liveness_meta"]["subsystems"];
    assert_eq!(subsystems["shown"], 3);
    assert_eq!(subsystems["total"], 3);
    assert_eq!(
        subsystems["truncated"], false,
        "an artifact that holds everything must say so, or the marker means \
         nothing on the artifacts that do not"
    );
    assert_eq!(
        subsystems["dropped_no_area"], 0,
        "nothing was dropped by the area filter here"
    );

    let important = &map["liveness_meta"]["important_files"];
    assert_eq!(important["shown"], 2);
    assert_eq!(important["total"], 2);
    assert_eq!(important["truncated"], false);
}

/// The area filter is a second, independent narrowing, and `total - shown` on
/// its own would blame the cap for it. A community whose representative file
/// sits at the repository root has no directory area to join against, so it is
/// dropped — correctly — and the drop is counted separately from the cut.
#[test]
fn subsystems_dropped_by_the_area_filter_are_counted_apart_from_the_cap() {
    let (mut extractions, mut communities) = packages(40);
    // Three members, so this sorts ahead of every two-member community and is
    // certainly inside the cap; all at the root, so `file_area` yields ".".
    let rootless = vec![
        "top_a.py".to_string(),
        "top_b.py".to_string(),
        "top_c.py".to_string(),
    ];
    for member in &rootless {
        extractions.push(extract_file(member, "def one():\n    return 1\n"));
    }
    communities.push(community(999, &rootless));
    extractions.extend(plans(30));
    let map = render(&extractions, communities);

    let subsystems = &map["liveness_meta"]["subsystems"];
    assert_eq!(subsystems["total"], 41);
    assert_eq!(
        subsystems["dropped_no_area"], 1,
        "the root-level community was filtered out after the cap admitted it"
    );
    assert_eq!(
        subsystems["shown"], 19,
        "20 admitted by the cap, one dropped by the filter"
    );
    assert_eq!(
        subsystems["shown"].as_u64().unwrap() as usize,
        len_of(&map, "subsystems")
    );
    assert_eq!(subsystems["truncated"], true);
}

/// R7's other half. Counting a cut honestly does not make the cut right: with
/// the list ordered by path alone, fifteen `docs/*_PLAN.md` files sort ahead of
/// `package.json` and `pyproject.toml` and take the whole cap, so the first
/// thing an agent reads to orient itself in a repository is fifteen plan
/// documents and none of the files that say what the repository *is*.
#[test]
fn important_files_rank_the_named_manifests_ahead_of_plan_documents() {
    let mut extractions: Vec<Extraction> = (0..15)
        .map(|index| extract_file(&format!("docs/aaa{index:03}_PLAN.md"), "# plan\n"))
        .collect();
    extractions.push(extract_file("package.json", "{}\n"));
    extractions.push(extract_file("pyproject.toml", "[project]\n"));
    let map = render(&extractions, Vec::new());

    let shown: Vec<&str> = map["important_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(shown.len(), 15, "the cap still holds");
    assert!(
        shown.contains(&"package.json") && shown.contains(&"pyproject.toml"),
        "the repository's own manifests must survive a cut that plan documents \
         would otherwise fill: {shown:?}"
    );
    assert_eq!(
        &shown[..2],
        ["package.json", "pyproject.toml"],
        "ranked ahead, not merely present: {shown:?}"
    );
    assert_eq!(map["liveness_meta"]["important_files"]["total"], 17);
    assert_eq!(map["liveness_meta"]["important_files"]["truncated"], true);
}
