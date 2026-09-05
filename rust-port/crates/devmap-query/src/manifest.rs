use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::artifacts::write_atomic;
use crate::model::*;
use devmap_analyze::model::*;
use devmap_extract::model::*;
use devmap_resolve::model::ResolvedEdge;
use serde_json::{json, Value};

pub(crate) const CONSUMER_MAP_ENGINE: &str = "devmap-rust";
const DEAD_CANDIDATE_CAP: usize = 200;
/// Unwired candidates share the dead-symbol cap: both are debt lists read by an
/// agent for orientation, both are budget-bearing, and a second constant would
/// only let the two drift. The true total travels beside the list in
/// `liveness_meta.unwired`, so the cap costs no information.
const UNWIRED_CANDIDATE_CAP: usize = DEAD_CANDIDATE_CAP;
const DEPENDENTS_CAP: usize = 1_024;
/// Entry roots the lean manifest carries before the token budget bites.
const ENTRY_ROOT_CAP: usize = 20;
/// Subsystems the lean manifest carries.
const SUBSYSTEM_CAP: usize = 20;
/// Important files the lean manifest carries.
const IMPORTANT_FILE_CAP: usize = 15;

pub fn generate_manifest(
    extractions: &[Extraction],
    analysis: &AnalysisSummary,
    freshness: FreshnessInfo,
) -> (Manifest, String) {
    let mut manifest = lean_manifest(extractions, analysis, freshness);
    let max_json_bytes = Budget::MANIFEST as usize * 4;
    let mut json_str = serde_json::to_string_pretty(&manifest).unwrap_or_default();
    while json_str.len() > max_json_bytes {
        if manifest.subsystems.pop().is_none()
            && manifest.entry_roots.pop().is_none()
            && manifest.important_files.pop().is_none()
        {
            break;
        }
        json_str = serde_json::to_string_pretty(&manifest).unwrap_or_default();
    }
    (manifest, json_str)
}

/// T1-bounded JSON used by token-budget tests. Identical to `generate_manifest`.
pub fn generate_lean_manifest_json(
    extractions: &[Extraction],
    analysis: &AnalysisSummary,
    freshness: FreshnessInfo,
) -> String {
    generate_manifest(extractions, analysis, freshness).1
}

/// Consumer-schema map: the keys Python `repo_map.json` readers already look
/// up (`files`, `dependents`, `dead_symbol_candidates`, `liveness_meta`, …).
/// Not token-budgeted — agents need the file list, not a 2k-token sketch.
pub fn generate_manifest_with_edges(
    extractions: &[Extraction],
    analysis: &AnalysisSummary,
    freshness: FreshnessInfo,
    edges: &[ResolvedEdge],
) -> (Manifest, String) {
    let (lean, _) = generate_manifest(extractions, analysis, freshness.clone());
    let json = consumer_manifest_json(extractions, analysis, &freshness, &lean, edges);
    (lean, json)
}

/// Whether wiring evidence marks this file as an entry point.
///
/// One owner for the rule. `lean_manifest` truncates its answer to fit a token
/// budget and `code_graph.json` carries it uncapped, so having each derive
/// "what is an entry root" separately is how the two artifacts start
/// disagreeing about the same repository.
pub(crate) fn is_entry_root(ext: &Extraction) -> bool {
    ext.wiring.iter().any(|w| {
        matches!(
            w.kind,
            WiringKind::ScriptEntry | WiringKind::FrameworkDecorator
        )
    })
}

/// Every entry-root file path, sorted and uncapped.
pub(crate) fn entry_root_paths(extractions: &[Extraction]) -> Vec<String> {
    let mut roots: Vec<String> = extractions
        .iter()
        .filter(|ext| is_entry_root(ext))
        .map(|ext| ext.file_path.clone())
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// Where an important file sits in the ranking, or `None` if it is not one.
///
/// One owner for both halves of the rule, for the same reason `is_entry_root`
/// has one: the list is truncated and the truncation has to report a
/// *pre-truncation* total, so membership has to be applicable to the corpus a
/// second time. A copy of the `ends_with("PLAN.md")` chain beside the counter
/// is how the count and the list come to disagree about what they are counting,
/// and a separate predicate beside a separate ranking is how a file comes to be
/// in the list with no rank, or ranked and not in the list.
///
/// The list is cut at [`IMPORTANT_FILE_CAP`], so ordering it by path alone was
/// R7 inverted in the same shape `dead_symbol_candidates` was: fifteen
/// `docs/*_PLAN.md` files sort ahead of `package.json` and `pyproject.toml`
/// and take the whole cap, and the first thing an agent reads to orient itself
/// in a repository is fifteen plan documents and nothing that says what the
/// repository *is*. The four named manifests answer "what is this project and
/// how is it built"; the plan glob is unbounded and answers something narrower,
/// so the named ones rank first and the glob fills what is left.
fn important_file_rank(path: &str) -> Option<u8> {
    match path {
        "README.md" | "Cargo.toml" | "package.json" | "pyproject.toml" => Some(0),
        _ if path.ends_with("PLAN.md") => Some(1),
        _ => None,
    }
}

/// Every important file path, ranked and uncapped.
///
/// Ranked, then deduplicated by a total order — `(rank, path)` — so two builds
/// of one generation emit the same list in the same order (R4).
fn important_file_paths(extractions: &[Extraction]) -> Vec<String> {
    let mut files: Vec<(u8, String)> = extractions
        .iter()
        .filter_map(|ext| {
            important_file_rank(&ext.file_path).map(|rank| (rank, ext.file_path.clone()))
        })
        .collect();
    files.sort();
    files.dedup();
    files.into_iter().map(|(_, path)| path).collect()
}

fn lean_manifest(
    extractions: &[Extraction],
    analysis: &AnalysisSummary,
    freshness: FreshnessInfo,
) -> Manifest {
    let mut subsystems = Vec::new();
    let mut entry_roots = Vec::new();
    let mut important_files = Vec::new();

    entry_roots.extend(entry_root_paths(extractions));
    important_files.extend(important_file_paths(extractions));

    entry_roots.truncate(ENTRY_ROOT_CAP);
    important_files.truncate(IMPORTANT_FILE_CAP);

    let mut sorted_comms = analysis.communities.clone();
    sorted_comms.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.members.cmp(&b.members))
            .then_with(|| a.name.cmp(&b.name))
    });

    for comm in sorted_comms.into_iter().take(SUBSYSTEM_CAP) {
        if let Some(first_member) = comm.members.first() {
            subsystems.push(SubsystemEntry {
                name: comm.name.clone(),
                path: first_member.clone(),
                entry_points: comm.members.iter().take(2).cloned().collect(),
            });
        }
    }

    Manifest {
        subsystems,
        entry_roots,
        important_files,
        freshness,
    }
}

/// Findings per Python `Confidence` tier.
///
/// Keyed through `code_graph.rs::confidence_label`, the one owner of the
/// numeric-to-tier mapping, so `repo_map.json` and `code_graph.json` can never
/// disagree about which tier a finding is in. Every tier is emitted, zero
/// included: an absent key would read as "not counted", and "no confident
/// findings" is a result worth stating.
fn confidence_histogram<'a>(
    reports: impl Iterator<Item = &'a DeadSymbolReport>,
) -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> =
        [("extracted", 0), ("inferred", 0), ("ambiguous", 0)]
            .into_iter()
            .collect();
    for report in reports {
        *counts
            .entry(crate::code_graph::confidence_label(report.confidence))
            .or_insert(0) += 1;
    }
    counts
}

fn consumer_manifest_json(
    extractions: &[Extraction],
    analysis: &AnalysisSummary,
    freshness: &FreshnessInfo,
    lean: &Manifest,
    edges: &[ResolvedEdge],
) -> String {
    let mut languages: BTreeSet<String> = BTreeSet::new();
    let mut files = Vec::new();
    for ext in extractions {
        if !ext.language.is_empty() && ext.language != "unknown" {
            languages.insert(ext.language.clone());
        }
        // No `summary` key. It was emitted as a constant `""` on every one of
        // these entries — 1,306 of them on this repository — and nothing reads
        // it: not the Python consumers, not the agent guides, not the visualizer
        // (which reads `summary` off *subsystems*, a different structure that
        // keeps its own). A field that always holds the same empty value is
        // bytes in a file agents are told to open, and nothing else.
        files.push(json!({
            "path": ext.file_path,
            "area": file_area(&ext.file_path),
            "kind": "code",
            "language": ext.language,
        }));
    }
    files.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap_or("")
            .cmp(right["path"].as_str().unwrap_or(""))
    });

    let (dependents, dependents_total) = build_dependents(edges);
    // R7: rank, then truncate. This took the first `DEAD_CANDIDATE_CAP` in
    // `analyze()`'s extraction order, which is file order — so 250 findings at
    // 0.4 declared before 5 at 0.9 filled the list and every confident one fell
    // off the end. The counts were honest (`{shown: 200, total: 255,
    // truncated: true}`); the cut was not, and a consumer told to prefer the
    // `extracted` tier saw none of it.
    //
    // Sorted by confidence descending, then by the identity itself so two
    // builds of one generation are byte-identical. `latest_dead_symbols` in the
    // store already sorts `is_exempt, confidence DESC, …`; this is that rule
    // applied to the path that reads the analysis blob instead.
    let mut ranked: Vec<&_> = analysis
        .dead_symbols
        .iter()
        .filter(|report| !report.is_exempt)
        .collect();
    ranked.sort_by(|left, right| {
        confidence_millis(right.confidence)
            .cmp(&confidence_millis(left.confidence))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
    });
    let dead_symbol_total = ranked.len();
    let dead_by_confidence_total = confidence_histogram(ranked.iter().copied());
    let dead_symbol_candidates: Vec<String> = ranked
        .iter()
        .take(DEAD_CANDIDATE_CAP)
        .map(|report| format!("{}::{}", report.file_path, report.symbol_name))
        .collect();
    let dead_by_confidence_shown =
        confidence_histogram(ranked.iter().copied().take(DEAD_CANDIDATE_CAP));

    // A subsystem `area` must be a real directory prefix, not a cluster label.
    //
    // `files[].area` is the file's parent directory (`file_area`) while this
    // used `entry.name`, which clustering generates as `community-1`. The two
    // never joined: `subsystem_map.area_for_path` matches an area against a
    // path, so every lookup silently found nothing, and the artifact contract
    // ("some file lives under this area") was violated for every entry.
    //
    // Derived from the subsystem's own representative file so both fields come
    // from one rule. An area with no file under it is dropped rather than
    // emitted empty — a subsystem nothing belongs to is not a subsystem, and
    // emitting it would keep the join broken while looking populated.
    let file_paths: BTreeSet<&str> = extractions
        .iter()
        .map(|ext| ext.file_path.as_str())
        .collect();
    let mut seen_areas: BTreeSet<String> = BTreeSet::new();
    let kept: Vec<(&SubsystemEntry, String)> = lean
        .subsystems
        .iter()
        .filter_map(|entry| {
            let area = file_area(&entry.path);
            if area == "." || !seen_areas.insert(area.clone()) {
                return None;
            }
            let prefix = format!("{area}/");
            if !file_paths.iter().any(|path| path.starts_with(&prefix)) {
                return None;
            }
            Some((entry, area))
        })
        .collect();
    // The areas a consumer can resolve a path to, longest first — the order
    // `subsystem_map.area_for_path` matches in, so adjacency is computed in the
    // vocabulary the lookup will use rather than in a second one beside it.
    let mut lookup_areas: Vec<String> = kept.iter().map(|(_, area)| area.clone()).collect();
    lookup_areas.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    let (adjacency, unresolved_endpoints) = area_adjacency(&lookup_areas, &file_paths, edges);
    let mut neighbors_shown = 0usize;
    let mut neighbors_total = 0usize;
    let subsystems: Vec<Value> = kept
        .iter()
        .map(|(entry, area)| {
            let coupled = adjacency.get(area);
            let total = coupled.map_or(0, BTreeMap::len);
            // Ranked by how many coupling edges run between the two areas, then
            // by name so a tie is not decided by hash order (R4). Emitted in
            // rank order rather than alphabetically: membership is what the
            // policy gate asks, but a reader deciding which coupling matters
            // needs the strongest first, and the cap has to cut the weakest.
            let mut ranked: Vec<(&String, &usize)> =
                coupled.map(|map| map.iter().collect()).unwrap_or_default();
            ranked.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
            let names: Vec<&str> = ranked
                .iter()
                .take(SUBSYSTEM_NEIGHBOR_CAP)
                .map(|(name, _)| name.as_str())
                .collect();
            neighbors_shown += names.len();
            neighbors_total += total;
            json!({
                "area": area,
                "summary": "",
                "entry_points": entry.entry_points,
                "critical_files": [entry.path],
                "neighbors": names,
                "handoff_paths": [],
                "role_files": {},
            })
        })
        .collect();
    // Two independent narrowings, reported as two numbers. `SUBSYSTEM_CAP` cuts
    // the ranked communities; the filter above then drops the ones with no
    // directory area to join against. `total - shown` alone would blame the cap
    // for both, and a reader deciding whether to ask for a bigger map needs to
    // know which one it was.
    let subsystems_total = analysis.communities.len();
    let subsystems_dropped_no_area = lean.subsystems.len().saturating_sub(subsystems.len());
    let important_files_total = important_file_paths(extractions).len();

    let (graph_degraded, graph_degraded_reason) = match &analysis.status {
        AnalysisStatus::Ok => (false, String::new()),
        AnalysisStatus::Partial { reason } => (true, format!("partial: {reason}")),
        AnalysisStatus::Timeout { reason } => (true, format!("timeout: {reason}")),
    };
    let entry_root_total = entry_root_paths(extractions).len();
    let all_unwired = crate::code_graph::unwired_candidates(extractions, edges);
    let unwired_excluded = all_unwired.excluded_coverage_loss;
    let unwired_total = all_unwired.paths.len();
    let unwired_shown: Vec<String> = all_unwired
        .paths
        .into_iter()
        .take(UNWIRED_CANDIDATE_CAP)
        .collect();
    let payload = json!({
        "languages": languages.into_iter().collect::<Vec<_>>(),
        "frameworks": [],
        "package_managers": [],
        "test_commands": [],
        "important_files": lean.important_files,
        "candidate_files": [],
        "files": files,
        "subsystems": subsystems,
        "dependents": dependents,
        "dependents_total": dependents_total,
        // Caller-supplied when available, empty otherwise — the same rule the
        // code graph applies, from the same `FreshnessInfo`, so the two
        // artifacts cannot disagree about how fresh they are.
        "generated_head": freshness.generated_head(),
        "indexed_hash": freshness.stamped.indexed_hash.clone().unwrap_or_default(),
        "content_fingerprint": freshness
            .stamped
            .content_fingerprint
            .clone()
            .unwrap_or_default(),
        // Derived from the same `AnalysisSummary` `code_graph.json` renders as
        // `analysis_status`, because the two artifacts of one build must not
        // disagree about whether the graph settled. These were literals, and
        // `RepoMapper.map_is_stale`'s fail-closed branch
        // (`if bool(repo_map.get("graph_degraded")): return True`) could
        // therefore never fire: a map built from a partition that never
        // converged was accepted as healthy by `--if-stale`, `watch` and
        // `verify`.
        "graph_degraded": graph_degraded,
        "graph_degraded_reason": graph_degraded_reason,
        "lsp": {},
        "dependency_risks": [],
        "entry_roots": lean.entry_roots,
        // Computed, not asserted empty. `code_graph.json` has always derived
        // this from the same `extractions` and `edges` this function already
        // receives; emitting `[]` here made the two artifacts contradict each
        // other, and read to a consumer as "nothing is unwired".
        "unwired_candidates": unwired_shown,
        // Genuinely never computed by this kernel — see
        // `liveness_meta.unavailable.unreachable_files` and the now
        // unconditional `liveness_unreachable_unreliable`, which is how
        // `code_graph.json` has always stated it.
        "unreachable_files": [],
        "dead_symbol_candidates": dead_symbol_candidates,
        "liveness_unreachable_unreliable": true,
        "liveness_meta": {
            "engine": CONSUMER_MAP_ENGINE,
            "dead_symbol": {
                "shown": dead_symbol_candidates.len(),
                "total": dead_symbol_total,
                "truncated": dead_symbol_total > dead_symbol_candidates.len(),
                // Retained under its original name for readers that predate
                // the shown/total split; it has always meant the true total.
                "count": dead_symbol_total,
                // The tier the flat `dead_symbol_candidates` strings cannot
                // carry, kept as a per-tier census over both populations
                // rather than by reshaping the list: `RepoMap` declares
                // `dead_symbol_candidates: List[str]`
                // (`repo_mapper.py:134`), and six `model_validate` call sites
                // would raise on objects, taking the whole map with them.
                //
                // Two histograms, not one, because the pair is what proves the
                // truncation ranked before it cut: equal `extracted` counts
                // mean no confident finding fell off the end. `CLAUDE.md`
                // tells agents to act on `extracted` and treat `inferred` as
                // unconfirmed, so that is the number they need.
                "by_confidence": {
                    "shown": dead_by_confidence_shown,
                    "total": dead_by_confidence_total,
                },
            },
            // `entry_roots` is capped at ENTRY_ROOT_CAP to hold the token
            // budget, so `count` — which was the post-truncation length —
            // reported the cap as the total. `is_entry_root` in
            // `subsystem_map.py` reads the capped list, so every genuine entry
            // root sorting after the cap was answered `false`; `truncated`
            // is what lets a consumer tell that answer is not knowable here.
            "entry_roots": {
                "shown": lean.entry_roots.len(),
                "total": entry_root_total,
                "truncated": entry_root_total > lean.entry_roots.len(),
            },
            // The two peers `entry_roots` was disclosed without. Both lists are
            // cut — `subsystems` at `SUBSYSTEM_CAP`, `important_files` at
            // `IMPORTANT_FILE_CAP`, and both again by `generate_manifest`
            // popping entries to hold the byte budget — and until now the only
            // keys that mentioned either were the lists themselves. An agent is
            // told to navigate this repository by `subsystems`; a repository
            // with 400 of them and one with 20 handed it the same artifact.
            //
            // `shown` counts what was emitted, not what the cap admitted, so it
            // stays true through every later drop.
            "subsystems": {
                "shown": subsystems.len(),
                "total": subsystems_total,
                "truncated": subsystems_total > subsystems.len(),
                "dropped_no_area": subsystems_dropped_no_area,
                // Provenance for `subsystems[].neighbors`. The field was a
                // literal `[]` for its whole life here, and an empty list is
                // exactly what a repository with no coupling would also
                // produce — so `backend/go_orchestrator/repomap` stopped
                // reading it and derives adjacency itself, saying in its
                // package doc that a consumer "cannot tell 'this repository
                // has no adjacent subsystems' from 'this producer does not
                // compute the field'". This flag is that distinction, and the
                // counts are the usual shown/total pair for a capped list.
                "neighbors_computed": true,
                "neighbors_shown": neighbors_shown,
                "neighbors_total": neighbors_total,
                "neighbors_truncated": neighbors_total > neighbors_shown,
                // Coupling edges whose endpoint named nothing this generation
                // indexed, so no area could be assigned to it. Zero on every
                // corpus measured; carried because "no neighbours" and "some
                // couplings could not be placed" are different answers.
                "neighbors_endpoints_unresolved": unresolved_endpoints,
            },
            "important_files": {
                "shown": lean.important_files.len(),
                "total": important_files_total,
                "truncated": important_files_total > lean.important_files.len(),
            },
            "unwired": {
                "shown": unwired_shown.len(),
                "total": unwired_total,
                "truncated": unwired_total > unwired_shown.len(),
                // `total` counts the population the filter left behind, so on
                // its own it is an honest number over a quietly narrowed set.
                // A file whose imports were never extracted cannot answer
                // "does anything import it", so it is excluded — and how many
                // were excluded travels with the count that would otherwise
                // read as the whole story.
                "excluded_coverage_loss": unwired_excluded,
            },
            "unavailable": {
                "unreachable_files": "file-level reachability BFS is not \
                     implemented in the Rust kernel; the empty list is not a \
                     computed result",
            },
        },
        "processes": [],
        "map_engine": CONSUMER_MAP_ENGINE,
        "freshness": freshness,
    });
    // Compact, not pretty. This artifact is not read by a person: it is the
    // file the agent guides instruct an agent to open before searching, and on
    // this repository indentation and newlines were 22% of it — 23,000 tokens
    // of whitespace in a 105,000-token file.
    //
    // It is not, however, evicting content: the *lean* manifest that feeds this
    // one is separately budgeted at 8,000 bytes and measures 5,524 pretty, so
    // nothing was being dropped to make room for the formatting. This is a size
    // win, not a recovery — `code_graph.json` was compacted for the same reason
    // earlier in this pass.
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(all(test, feature = "parse"))]
mod wire_format_tests {
    use super::*;
    use devmap_extract::extract_file;

    fn manifest_json() -> String {
        let extractions = vec![
            extract_file("src/a.py", "def one():\n    return 1\n"),
            extract_file("src/b.py", "def two():\n    return 2\n"),
        ];
        let analysis = AnalysisSummary {
            // No discovery step ran over this hand-built corpus, so there is no
            // refusal count to report. `None` says that; `0` would claim a walk.
            discovery_refused_files: None,
            total_files: 2,
            total_symbols: 2,
            total_edges: 0,
            dead_symbols: Vec::new(),
            communities: Vec::new(),
            status: AnalysisStatus::Ok,
            unresolved_calls: 0,
            clone_coverage: Default::default(),
        };
        let freshness = FreshnessInfo::new("head".into(), 1, 0);
        let (_, json) = generate_manifest_with_edges(&extractions, &analysis, freshness, &[]);
        json
    }

    /// The artifact agents are told to open must not spend a quarter of itself
    /// on indentation. Measured on this repository, pretty-printing was 22% of
    /// `repo_map.json`; with the constant `summary` field it came to 26.2%,
    /// 27,578 tokens.
    #[test]
    fn the_consumer_manifest_is_not_pretty_printed() {
        let json = manifest_json();
        assert!(
            !json.contains("\n  \""),
            "the consumer manifest is indented; this is a file for a machine to \
             read and the whitespace is a fifth of it"
        );
        // Still valid JSON, and still the schema consumers index into.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        for key in [
            "files",
            "dependents",
            "subsystems",
            "entry_roots",
            "map_engine",
        ] {
            assert!(parsed.get(key).is_some(), "consumer key {key} disappeared");
        }
    }

    /// `summary` was emitted as a constant empty string on every file entry.
    /// Nothing read it. Subsystems keep theirs, because the visualizer indexes
    /// it directly and would raise on its absence.
    #[test]
    fn file_entries_carry_no_constant_summary_but_subsystems_keep_theirs() {
        let parsed: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
        let files = parsed["files"].as_array().expect("files is an array");
        assert!(!files.is_empty(), "fixture produced no files");
        for file in files {
            assert!(
                file.get("summary").is_none(),
                "a constant empty summary is back on file entries: {file}"
            );
            for key in ["path", "area", "kind", "language"] {
                assert!(file.get(key).is_some(), "file entry lost {key}: {file}");
            }
        }
        for subsystem in parsed["subsystems"].as_array().unwrap_or(&Vec::new()) {
            assert!(
                subsystem.get("summary").is_some(),
                "subsystem summary was removed; map_viz.py indexes it directly"
            );
        }
    }
}

fn file_area(path: &str) -> String {
    Path::new(path)
        .parent()
        .and_then(|parent| parent.to_str())
        .filter(|parent| !parent.is_empty() && *parent != ".")
        .unwrap_or(".")
        .replace('\\', "/")
}

/// How many neighbours one subsystem may name.
///
/// A cap, because `area_for_path` falls back to a file's own parent directory
/// when no subsystem prefix matches it, so a repository with a wide flat tree
/// can couple one area to hundreds. What is cut is reported beside it.
const SUBSYSTEM_NEIGHBOR_CAP: usize = 32;

/// Which areas are coupled, and how strongly, from the generation's own edges.
///
/// The same derivation `backend/go_orchestrator/repomap` performs, deliberately:
/// that package gave up on `subsystems[].neighbors` because this writer emitted
/// a literal `[]`, and two implementations of one relation are how the map and
/// its readers come to disagree about repository structure. If the two must
/// coexist they must at least compute the same thing.
///
/// - Only `calls`, `references` and `imports` couple two areas. `contains`,
///   `defines` and `member_of` are structural relations inside a file and say
///   nothing about one area depending on another.
/// - Only edges at `extracted` confidence. An `ambiguous` edge is a resolution
///   the analyser explicitly declined to make, and this relation is read by the
///   write gate to *widen* what a task may touch — a scope decision resting on
///   a guess is the opposite of evidence. Measured on this repository by the Go
///   side: ambiguous edges account for 288 of 431 linked area pairs, so
///   admitting them roughly triples the neighbourhood.
/// - Symmetric: a reference in one direction is a coupling in both.
///
/// Areas are resolved the way `subsystem_map.area_for_path` resolves them — the
/// longest declared subsystem area that prefixes the file, else the file's own
/// parent directory — because a relation keyed by a vocabulary the lookup does
/// not use is an empty relation with extra steps. That exact mismatch
/// (`community-4` against a directory path) is why the field joined nothing
/// before it was a literal.
/// Returns the adjacency and the number of coupling edges whose endpoint could
/// not be placed in any area.
fn area_adjacency(
    lookup_areas: &[String],
    file_paths: &BTreeSet<&str>,
    edges: &[ResolvedEdge],
) -> (BTreeMap<String, BTreeMap<String, usize>>, usize) {
    // Not every endpoint is a file. The resolver emits a synthetic node for
    // each Go package (`package:<dir>/<pkg>`) and points the package's imports
    // at it, so `backend/.../dcgrep` came out coupled to both
    // `backend/go_orchestrator/internal/proc` and
    // `package:backend/go_orchestrator/internal/proc` — the same coupling
    // twice, once in a vocabulary `area_for_path` can never produce, spending a
    // slot of the cap to say it.
    //
    // Resolved through the graph rather than by knowing how the node is spelled:
    // each file of a package carries a `MemberOf` edge to its package node, so
    // the node's own area is the area of any member. A generation this reads
    // holds those edges by construction.
    let mut member_file: BTreeMap<&str, &str> = BTreeMap::new();
    for edge in edges {
        if edge.edge_kind == EdgeKind::MemberOf
            && !file_paths.contains(edge.target_file.as_str())
            && file_paths.contains(edge.source_file.as_str())
        {
            member_file
                .entry(edge.target_file.as_str())
                .or_insert(edge.source_file.as_str());
        }
    }
    // One memo for the sweep. A generation has thousands of files and hundreds
    // of thousands of edges, so resolving the area per edge would repeat the
    // prefix scan 271,000 times on the benchmark corpus. Keyed by an owned
    // string: the map outlives each edge's borrow, and the miss path allocates
    // once per distinct endpoint rather than once per edge.
    let mut area_of: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut adjacency: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut unresolved = 0usize;
    for edge in edges {
        if !matches!(
            edge.edge_kind,
            EdgeKind::Calls | EdgeKind::References | EdgeKind::Imports
        ) {
            continue;
        }
        if crate::code_graph::confidence_label(edge.confidence.0) != "extracted" {
            continue;
        }
        if edge.source_file == edge.target_file {
            continue;
        }
        let (Some(from), Some(to)) = (
            endpoint_area(
                &edge.source_file,
                lookup_areas,
                file_paths,
                &member_file,
                &mut area_of,
            ),
            endpoint_area(
                &edge.target_file,
                lookup_areas,
                file_paths,
                &member_file,
                &mut area_of,
            ),
        ) else {
            // An endpoint naming nothing this generation indexed. Counted, not
            // dropped in silence: a relation missing an edge and a relation
            // that found none are different answers.
            unresolved += 1;
            continue;
        };
        if from == to {
            continue;
        }
        *adjacency
            .entry(from.clone())
            .or_default()
            .entry(to.clone())
            .or_default() += 1;
        *adjacency.entry(to).or_default().entry(from).or_default() += 1;
    }
    (adjacency, unresolved)
}

/// The area one edge endpoint belongs to, or `None` when it belongs to nothing
/// this generation indexed.
fn endpoint_area(
    endpoint: &str,
    lookup_areas: &[String],
    file_paths: &BTreeSet<&str>,
    member_file: &BTreeMap<&str, &str>,
    memo: &mut BTreeMap<String, Option<String>>,
) -> Option<String> {
    if let Some(known) = memo.get(endpoint) {
        return known.clone();
    }
    let answer = if file_paths.contains(endpoint) {
        Some(resolved_area(endpoint, lookup_areas))
    } else {
        member_file
            .get(endpoint)
            .map(|file| resolved_area(file, lookup_areas))
    };
    memo.insert(endpoint.to_string(), answer.clone());
    answer
}

/// `subsystem_map.area_for_path`, in this kernel: the longest declared
/// subsystem area that prefixes the path, and the file's own parent directory
/// when none does. `lookup_areas` must already be longest-first.
fn resolved_area(path: &str, lookup_areas: &[String]) -> String {
    for area in lookup_areas {
        if path == area
            || (path.len() > area.len()
                && path.as_bytes()[area.len()] == b'/'
                && path.starts_with(area.as_str()))
        {
            return area.clone();
        }
    }
    file_area(path)
}

fn build_dependents(
    edges: &[ResolvedEdge],
) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, usize>) {
    let mut importers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in edges {
        if edge.edge_kind != EdgeKind::Imports {
            continue;
        }
        if edge.source_file == edge.target_file {
            continue;
        }
        importers
            .entry(edge.target_file.clone())
            .or_default()
            .insert(edge.source_file.clone());
    }
    let mut dependents = BTreeMap::new();
    let mut totals = BTreeMap::new();
    for (target, sources) in importers {
        let total = sources.len();
        let listed: Vec<String> = sources.into_iter().take(DEPENDENTS_CAP).collect();
        if total > listed.len() {
            totals.insert(target.clone(), total);
        }
        dependents.insert(target, listed);
    }
    (dependents, totals)
}

/// Join a relative default output to the indexed repo root. Absolute paths
/// are left unchanged so `--output /tmp/map.json` still works.
pub fn resolve_manifest_output(repo_root: Option<&str>, output: &Path) -> PathBuf {
    if output.is_absolute() {
        return output.to_path_buf();
    }
    match repo_root {
        Some(root) if !root.is_empty() => Path::new(root).join(output),
        _ => output.to_path_buf(),
    }
}

/// Refuse to clobber a Python (or otherwise foreign) `repo_map.json` unless
/// `force` is set. Identity is `map_engine == "devmap-rust"` — missing that
/// key is the live Python schema.
pub fn write_manifest_atomically(path: &Path, json: &str, force: bool) -> anyhow::Result<bool> {
    if path.exists() && !force && is_foreign_repo_map(path)? {
        anyhow::bail!(
            "refuse to overwrite a non-devmap-rust repo map at {} (pass --force to replace)",
            path.display()
        );
    }
    Ok(write_atomic(path, json.as_bytes())?)
}

fn is_foreign_repo_map(path: &Path) -> anyhow::Result<bool> {
    let existing = std::fs::read_to_string(path)?;
    let Ok(value) = serde_json::from_str::<Value>(&existing) else {
        return Ok(true);
    };
    Ok(value.get("map_engine").and_then(Value::as_str) != Some(CONSUMER_MAP_ENGINE))
}

#[cfg(all(test, feature = "parse"))]
mod tests {
    use super::*;
    use devmap_extract::extract_file;

    fn tmp_map(contents: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "devmap-manifest-{}-{stamp}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("repo_map.json");
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn freshness() -> FreshnessInfo {
        FreshnessInfo {
            head_sha: "abc123".to_string(),
            generation_id: 1,
            pending_count: 0,
            stamped: Default::default(),
        }
    }

    fn empty_analysis() -> AnalysisSummary {
        AnalysisSummary {
            // No discovery step ran over this hand-built corpus, so there is no
            // refusal count to report. `None` says that; `0` would claim a walk.
            discovery_refused_files: None,
            total_files: 0,
            total_symbols: 0,
            total_edges: 0,
            dead_symbols: Vec::new(),
            communities: Vec::new(),
            status: AnalysisStatus::Ok,
            unresolved_calls: 0,
            clone_coverage: Default::default(),
        }
    }

    /// The manifest's important-file list matches on equality, not inequality.
    ///
    /// `==` flipped to `!=` survived: nothing asserted *which* files land in
    /// `important_files`, so a list containing every file except the important
    /// ones read as correct. This is the first thing an agent reads to orient
    /// in a repository.
    #[test]
    fn important_files_are_the_named_ones_and_nothing_else() {
        let extractions = vec![
            extract_file("README.md", "# hi\n"),
            extract_file("Cargo.toml", "[package]\n"),
            extract_file("src/random.py", "def x(): pass\n"),
            extract_file("docs/PLAN.md", "# plan\n"),
        ];
        let manifest = lean_manifest(&extractions, &empty_analysis(), freshness());

        assert!(
            manifest.important_files.contains(&"README.md".to_string()),
            "README.md must be important: {:?}",
            manifest.important_files
        );
        assert!(manifest.important_files.contains(&"Cargo.toml".to_string()));
        assert!(
            manifest
                .important_files
                .contains(&"docs/PLAN.md".to_string()),
            "a PLAN.md suffix match must be important"
        );
        assert!(
            !manifest
                .important_files
                .contains(&"src/random.py".to_string()),
            "an ordinary source file must NOT be important: {:?}",
            manifest.important_files
        );
    }

    /// The JSON budget is a multiple of the token budget, and the JSON is real.
    ///
    /// `*` flipped to `+` survived because nothing exercised the shrink loop,
    /// and the whole body of `generate_lean_manifest_json` could be replaced
    /// with `String::new()` because nothing asserted it returned parseable
    /// JSON carrying the manifest's content.
    #[test]
    fn lean_manifest_json_is_real_parseable_output_within_budget() {
        // `PLAN.md` matches by suffix, so these land in `important_files`
        // (capped at 15). The paths are deliberately long: the lean manifest is
        // small by construction, and unless it exceeds MANIFEST+4 bytes the
        // fixture cannot tell `MANIFEST * 4` from `MANIFEST + 4`.
        let deep = "nested_directory_segment".repeat(8);
        let extractions: Vec<_> = (0..30)
            .map(|i| extract_file(&format!("src/{deep}/mod{i:04}/PLAN.md"), "# doc\n"))
            .collect();

        let json = generate_lean_manifest_json(&extractions, &empty_analysis(), freshness());
        assert!(!json.is_empty(), "the manifest JSON must not be empty");

        let value: Value = serde_json::from_str(&json).expect("manifest JSON must parse");
        assert_eq!(
            value.get("freshness").and_then(|f| f.get("head_sha")),
            Some(&Value::String("abc123".to_string())),
            "the manifest must carry the real freshness identity"
        );
        assert!(
            value.get("important_files").is_some(),
            "the manifest must carry its important_files key: {json}"
        );

        // The shrink loop bounds output at MANIFEST tokens x 4 bytes. With `+`
        // instead of `*` the bound collapses to roughly the token count, and a
        // manifest this size can no longer fit.
        let max = Budget::MANIFEST as usize * 4;
        assert!(
            json.len() <= max,
            "manifest JSON is {} bytes, over the {max}-byte budget",
            json.len()
        );
        assert!(
            json.len() > Budget::MANIFEST as usize + 4,
            "this manifest is {} bytes; it must exceed MANIFEST+4 or the test \
             cannot distinguish `MANIFEST * 4` from `MANIFEST + 4`",
            json.len()
        );
    }

    /// Language collection rejects empty and `unknown`, and needs both guards.
    ///
    /// `&&` flipped to `||` survived: with `||`, an empty language passes the
    /// `!= "unknown"` half and is admitted, so the manifest advertises a
    /// language that does not exist.
    #[test]
    fn manifest_languages_exclude_empty_and_unknown() {
        let mut blank = extract_file("a.txt", "x\n");
        blank.language = String::new();
        let mut unknown = extract_file("b.bin", "x\n");
        unknown.language = "unknown".to_string();
        let real = extract_file("c.py", "def x(): pass\n");

        let json = {
            let extractions = [blank, unknown, real];
            let analysis = empty_analysis();
            let fresh = freshness();
            let lean = lean_manifest(&extractions, &analysis, fresh.clone());
            consumer_manifest_json(&extractions, &analysis, &fresh, &lean, &[])
        };
        let value: Value = serde_json::from_str(&json).expect("consumer manifest parses");
        let languages: Vec<&str> = value["languages"]
            .as_array()
            .expect("languages array")
            .iter()
            .filter_map(Value::as_str)
            .collect();

        assert!(
            languages.contains(&"python"),
            "real languages are kept: {languages:?}"
        );
        assert!(
            !languages.contains(&""),
            "an empty language must never be advertised: {languages:?}"
        );
        assert!(
            !languages.contains(&"unknown"),
            "`unknown` must never be advertised: {languages:?}"
        );
    }

    /// Build the consumer manifest the way `generate_manifest_with_edges` does.
    fn consumer_json(
        extractions: &[Extraction],
        analysis: &AnalysisSummary,
        edges: &[ResolvedEdge],
    ) -> Value {
        let fresh = freshness();
        let lean = lean_manifest(extractions, analysis, fresh.clone());
        let json = consumer_manifest_json(extractions, analysis, &fresh, &lean, edges);
        serde_json::from_str(&json).expect("consumer manifest parses")
    }

    fn script_entry(path: &str) -> Extraction {
        let mut ext = extract_file(path, "def main(): pass\n");
        ext.wiring.push(devmap_extract::model::WiringAnnotation {
            kind: WiringKind::ScriptEntry,
            target_symbol: path.to_string(),
            details: "entry".to_string(),
        });
        ext
    }

    /// `repo_map.json` must not claim a healthy graph when the analysis degraded.
    ///
    /// `graph_degraded` was a literal `false` on every path while the same
    /// `AnalysisSummary` could say `Partial`, and `code_graph.json` — written in
    /// the same build from the same value — rendered it honestly. The consumer
    /// that matters is `RepoMapper.map_is_stale`
    /// (`src/devcouncil/indexing/repo_mapper.py`), whose fail-closed branch
    /// `if bool(repo_map.get("graph_degraded")): return True` could never fire,
    /// so `--if-stale`, `watch` and `verify` accepted a map built from a
    /// partition that never settled.
    #[test]
    fn a_degraded_analysis_is_not_reported_as_a_healthy_graph() {
        let extractions = vec![script_entry("main.py")];
        let mut analysis = empty_analysis();
        analysis.status = AnalysisStatus::Partial {
            reason: "louvain hit MAX_PASSES without converging".to_string(),
        };

        let value = consumer_json(&extractions, &analysis, &[]);

        assert_eq!(
            value["graph_degraded"], true,
            "a Partial analysis must degrade the map: {value}"
        );
        let reason = value["graph_degraded_reason"].as_str().unwrap_or("");
        assert!(
            reason.contains("louvain"),
            "the reason must name the cause, got {reason:?}"
        );

        // The healthy case must stay healthy, or the fail-closed branch becomes
        // an unconditional rebuild.
        let healthy = consumer_json(&extractions, &empty_analysis(), &[]);
        assert_eq!(healthy["graph_degraded"], false);
        assert_eq!(healthy["graph_degraded_reason"], "");
    }

    /// The two artifacts of one build must not contradict each other about
    /// liveness.
    ///
    /// `unwired_candidates` was a literal `[]` in `repo_map.json` while
    /// `code_graph.json` computed it from the same `extractions` and `edges`
    /// that `consumer_manifest_json` already receives. `unreachable_files` is
    /// never computed by this kernel at all, yet `repo_map.json` gated its
    /// unreliability flag on `entry_roots.is_empty()` — false on any repository
    /// with an entry root — while `code_graph.json` sets the same flag
    /// unconditionally.
    #[test]
    fn the_manifest_reports_liveness_it_computed_and_flags_what_it_did_not() {
        let orphan = extract_file("orphan.py", "def g(): pass\n");
        let used = extract_file("used.py", "def h(): pass\n");
        let app = extract_file("app.py", "import used\n");
        let extractions = vec![script_entry("main.py"), orphan, used, app];
        let edges = vec![import_edge("app.py", "used.py")];

        let value = consumer_json(&extractions, &empty_analysis(), &edges);

        let unwired: Vec<&str> = value["unwired_candidates"]
            .as_array()
            .expect("unwired_candidates array")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            unwired.contains(&"orphan.py"),
            "an unimported non-entry file is an unwired candidate: {unwired:?}"
        );
        assert!(
            !unwired.contains(&"main.py"),
            "an entry root is never unwired: {unwired:?}"
        );

        assert_eq!(
            value["liveness_unreachable_unreliable"], true,
            "file-level reachability is never computed here, so the flag is \
             unconditional — `code_graph.json` already sets it that way"
        );
        let marker = value["liveness_meta"]["unavailable"]["unreachable_files"]
            .as_str()
            .unwrap_or("");
        assert!(
            marker.contains("not"),
            "the empty `unreachable_files` needs a marker saying it was never \
             computed, got {marker:?}"
        );
    }

    /// A truncated list must not report its truncated length as the total.
    ///
    /// `entry_roots` is capped at 20 for the token budget, and
    /// `liveness_meta.entry_roots.count` was `lean.entry_roots.len()` — the
    /// post-truncation length. `code_graph.json` emits the same list uncapped,
    /// so a 25-root repository had one artifact saying 25 and the other 20.
    #[test]
    fn entry_root_count_is_the_true_total_not_the_truncated_length() {
        let extractions: Vec<Extraction> = (0..25)
            .map(|i| script_entry(&format!("svc{i:02}/main.py")))
            .collect();

        let value = consumer_json(&extractions, &empty_analysis(), &[]);

        assert_eq!(
            value["entry_roots"].as_array().map(Vec::len),
            Some(20),
            "the list itself stays capped"
        );
        let meta = &value["liveness_meta"]["entry_roots"];
        assert_eq!(
            meta["total"], 25,
            "the count must be the real total, not the cap: {meta}"
        );
        assert_eq!(meta["shown"], 20);
        assert_eq!(meta["truncated"], true);
    }

    fn import_edge(source: &str, target: &str) -> ResolvedEdge {
        ResolvedEdge {
            source_file: source.to_string(),
            target_file: target.to_string(),
            source_symbol: format!("{source}::s"),
            target_symbol: format!("{target}::t"),
            edge_kind: EdgeKind::Imports,
            confidence: Confidence::DETERMINISTIC,
            resolution: None,
            details: None,
        }
    }

    /// `build_dependents` counts real importers, and only real ones.
    ///
    /// Mutation testing replaced this function wholesale and flipped every
    /// comparison in it without a failure — nothing asserted its output at all.
    /// It feeds the manifest's dependency counts, so a silent miscount there is
    /// a wrong answer a consumer cannot detect.
    #[test]
    fn dependents_counts_distinct_cross_file_importers() {
        let edges = vec![
            import_edge("a.py", "lib.py"),
            import_edge("b.py", "lib.py"),
            // Duplicate importer: a set, so it must count once.
            import_edge("a.py", "lib.py"),
            // Self-import must be skipped, or every file depends on itself.
            import_edge("lib.py", "lib.py"),
            // A non-Imports edge must not contribute.
            ResolvedEdge {
                edge_kind: EdgeKind::Calls,
                ..import_edge("c.py", "lib.py")
            },
        ];

        let (dependents, totals) = build_dependents(&edges);

        assert_eq!(
            dependents.get("lib.py").map(Vec::as_slice),
            Some(["a.py".to_string(), "b.py".to_string()].as_slice()),
            "only distinct cross-file importers over Imports edges count"
        );
        assert!(
            !dependents.contains_key("c.py") && !dependents.contains_key("a.py"),
            "a Calls edge must not create a dependency entry"
        );
        // Under the cap, so nothing is reported as truncated. The `total >
        // listed.len()` guard must be strict: reporting a total here would tell
        // a consumer the list was capped when it was complete.
        assert!(
            totals.is_empty(),
            "an uncapped list must not report a separate total: {totals:?}"
        );
    }

    #[test]
    fn dependents_reports_a_total_only_when_the_list_is_capped() {
        let edges: Vec<ResolvedEdge> = (0..DEPENDENTS_CAP + 5)
            .map(|i| import_edge(&format!("src{i:05}.py"), "lib.py"))
            .collect();

        let (dependents, totals) = build_dependents(&edges);

        assert_eq!(
            dependents["lib.py"].len(),
            DEPENDENTS_CAP,
            "the listed importers must be capped"
        );
        assert_eq!(
            totals.get("lib.py"),
            Some(&(DEPENDENTS_CAP + 5)),
            "a capped list must carry the true total — never present a capped \
             sample as complete coverage"
        );
    }

    /// A relative manifest path is resolved against a non-empty repo root only.
    ///
    /// The `!root.is_empty()` guard was replaceable with `true`: an empty root
    /// would then join to a bare relative path, silently writing the manifest
    /// somewhere other than intended.
    #[test]
    fn manifest_output_resolves_against_a_usable_repo_root() {
        let rel = Path::new("out/repo_map.json");
        assert_eq!(
            resolve_manifest_output(Some("/repo"), rel),
            Path::new("/repo/out/repo_map.json")
        );
        // An empty root is not a root.
        assert_eq!(resolve_manifest_output(Some(""), rel), rel.to_path_buf());
        assert_eq!(resolve_manifest_output(None, rel), rel.to_path_buf());
        // An absolute output ignores the root entirely.
        let abs = Path::new("/tmp/x/repo_map.json");
        assert_eq!(
            resolve_manifest_output(Some("/repo"), abs),
            abs.to_path_buf()
        );
    }

    /// The guard that stops devmap overwriting someone else's repo map.
    ///
    /// Mutation testing replaced this whole function with `Ok(true)` and no
    /// test noticed — meaning nothing pinned the *negative* case, which is the
    /// one that matters for usability: if it always answers "foreign", devmap
    /// can never refresh a map it wrote itself. The positive case matters for
    /// safety: the live `.devcouncil/repo_map.json` in a DevCouncil repo is
    /// generated by the frozen Python mapper and must never be clobbered.
    #[test]
    fn foreign_repo_map_detection_distinguishes_both_directions() {
        // Our own map: not foreign, so a refresh is allowed.
        let ours = tmp_map(&format!(r#"{{"map_engine": "{CONSUMER_MAP_ENGINE}"}}"#));
        assert!(
            !is_foreign_repo_map(&ours).unwrap(),
            "a map devmap wrote itself must be refreshable, or devmap can never \
             update its own output"
        );

        // Someone else's map: foreign, so it is protected.
        let theirs = tmp_map(r#"{"map_engine": "python-repo-mapper"}"#);
        assert!(
            is_foreign_repo_map(&theirs).unwrap(),
            "another engine's map must be protected from being overwritten"
        );

        // No engine marker at all — treat as foreign. Absence of proof that we
        // wrote it is not proof that we did.
        let unmarked = tmp_map(r#"{"files": []}"#);
        assert!(
            is_foreign_repo_map(&unmarked).unwrap(),
            "a map with no engine marker must be treated as foreign"
        );

        // Unparseable content is foreign too: a check that could not run must
        // not report the same result as a check that ran and passed.
        let broken = tmp_map("not json at all {{{");
        assert!(
            is_foreign_repo_map(&broken).unwrap(),
            "unreadable content must fail closed, not be assumed ours"
        );

        for path in [ours, theirs, unmarked, broken] {
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }
}
