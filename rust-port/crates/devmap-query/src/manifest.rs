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
const DEPENDENTS_CAP: usize = 1_024;

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

fn lean_manifest(
    extractions: &[Extraction],
    analysis: &AnalysisSummary,
    freshness: FreshnessInfo,
) -> Manifest {
    let mut subsystems = Vec::new();
    let mut entry_roots = Vec::new();
    let mut important_files = Vec::new();

    entry_roots.extend(entry_root_paths(extractions));

    for ext in extractions {
        if ext.file_path == "README.md"
            || ext.file_path == "Cargo.toml"
            || ext.file_path == "package.json"
            || ext.file_path == "pyproject.toml"
            || ext.file_path.ends_with("PLAN.md")
        {
            important_files.push(ext.file_path.clone());
        }
    }

    entry_roots.sort();
    important_files.sort();
    entry_roots.truncate(20);
    important_files.truncate(15);

    let mut sorted_comms = analysis.communities.clone();
    sorted_comms.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.members.cmp(&b.members))
            .then_with(|| a.name.cmp(&b.name))
    });

    for comm in sorted_comms.into_iter().take(20) {
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
    let dead_symbol_candidates: Vec<String> = analysis
        .dead_symbols
        .iter()
        .filter(|report| !report.is_exempt)
        .take(DEAD_CANDIDATE_CAP)
        .map(|report| format!("{}::{}", report.file_path, report.symbol_name))
        .collect();

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
    let subsystems: Vec<Value> = lean
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
            Some(json!({
                "area": area,
                "summary": "",
                "entry_points": entry.entry_points,
                "critical_files": [entry.path],
                "neighbors": [],
                "handoff_paths": [],
                "role_files": {},
            }))
        })
        .collect();

    let liveness_unreliable = lean.entry_roots.is_empty();
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
        "graph_degraded": false,
        "graph_degraded_reason": "",
        "lsp": {},
        "dependency_risks": [],
        "entry_roots": lean.entry_roots,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": dead_symbol_candidates,
        "liveness_unreachable_unreliable": liveness_unreliable,
        "liveness_meta": {
            "engine": CONSUMER_MAP_ENGINE,
            "dead_symbol": { "count": analysis.dead_symbols.iter().filter(|r| !r.is_exempt).count() },
            "entry_roots": { "count": lean.entry_roots.len() },
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
