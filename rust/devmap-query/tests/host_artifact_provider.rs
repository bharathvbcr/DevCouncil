use devmap_query::host::{
    ArtifactKind, ArtifactProvider, FilesystemArtifactProvider, DEFAULT_ARTIFACT_BYTES,
};
use devmap_query::viz::VizOptions;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn null_export_tier_preserves_legacy_compatibility_without_hiding_incompleteness() {
    let mut value = graph();
    value["meta"] = json!({"compatibility_export_tier": null});
    devmap_query::host::validate_artifact(ArtifactKind::CodeGraph, &value).unwrap();
    value["meta"]["graph_export_incomplete_reason"] = json!("edges were capped");
    let error = devmap_query::host::validate_artifact(ArtifactKind::CodeGraph, &value)
        .expect_err("an absent tier cannot conceal an incomplete graph");
    assert!(error.to_string().contains("edges were capped"));
}

fn scratch(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "devmap-host-provider-{label}-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("scratch directory");
    path
}

fn graph() -> Value {
    json!({
        "schema_version": devmap_query::CODE_GRAPH_SCHEMA_VERSION,
        "nodes": [
            {"id": "src/a.rs", "name": "a.rs", "kind": "file", "path": "src/a.rs"},
            {"id": "src/b.rs", "name": "b.rs", "kind": "file", "path": "src/b.rs"}
        ],
        "edges": [
            {"source": "src/a.rs", "target": "src/b.rs", "kind": "imports", "confidence": 1.0}
        ]
    })
}

fn repo_map() -> Value {
    json!({
        "map_engine": "devmap-rust",
        "files": [
            {"path": "src/a.rs", "area": "src", "kind": "module", "language": "rust"}
        ],
        "subsystems": [
            {"area": "src", "entry_points": ["src/a.rs"], "critical_files": ["src/a.rs"],
             "neighbors": [], "handoff_paths": [], "role_files": {}, "role_file_counts": {}}
        ],
        "entry_roots": [],
        "important_files": [],
        "unwired_candidates": [],
        "dead_symbol_candidates": [],
        "liveness_meta": {
            "engine": "devmap-rust",
            "dead_symbol": {"shown": 0, "total": 0, "truncated": false},
            "entry_roots": {"shown": 0, "total": 0, "truncated": false},
            "subsystems": {
                "shown": 1, "total": 1, "truncated": false,
                "role_files_shown": 0, "role_files_total": 0,
                "role_files_truncated": false
            },
            "important_files": {"shown": 0, "total": 0, "truncated": false},
            "unwired": {"shown": 0, "total": 0, "truncated": false},
            "unavailable": {}
        }
    })
}

fn write_json(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
    std::fs::write(path, serde_json::to_vec(value).expect("JSON")).expect("write fixture");
}

#[test]
fn one_filesystem_provider_reads_projects_and_renders_both_artifacts() {
    let root = scratch("both");
    let graph_path = root.join("graph.json");
    let map_path = root.join("map.json");
    write_json(&graph_path, &graph());
    write_json(&map_path, &repo_map());

    let provider = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&graph_path)
        .with_repo_map_path(&map_path);

    assert_eq!(provider.read_code_graph().unwrap(), graph());
    assert_eq!(provider.read_repo_map().unwrap(), repo_map());
    let graph_payload = provider.code_graph_payload(&VizOptions::default()).unwrap();
    assert_eq!(graph_payload["counts"]["nodes_total"], 2);
    assert_eq!(
        provider.repo_map_payload().unwrap()["nodes"][0]["id"],
        "src"
    );
    assert!(provider
        .code_graph_html(&VizOptions::default())
        .unwrap()
        .contains("2 nodes"));
    assert!(provider.repo_map_html().unwrap().contains("const DATA"));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn missing_invalid_oversized_and_wrong_shape_are_distinct_refusals() {
    let root = scratch("refusals");
    let missing = root.join("missing.json");
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&missing)
        .read_code_graph()
        .unwrap_err();
    assert_eq!(err.kind(), ArtifactKind::CodeGraph);
    assert!(err.to_string().contains("does not exist"));

    let invalid = root.join("invalid.json");
    std::fs::write(&invalid, b"{not json").unwrap();
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&invalid)
        .read_code_graph()
        .unwrap_err();
    assert!(err.to_string().contains("valid JSON"));

    let oversized = root.join("oversized.json");
    std::fs::write(
        &oversized,
        b"{\"schema_version\":2,\"nodes\":[],\"edges\":[]}",
    )
    .unwrap();
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&oversized)
        .with_max_bytes(8)
        .read_code_graph()
        .unwrap_err();
    assert!(err.to_string().contains("8-byte limit"));

    let wrong_shape = root.join("wrong-shape.json");
    write_json(
        &wrong_shape,
        &json!({"schema_version": 2, "nodes": {}, "edges": []}),
    );
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&wrong_shape)
        .read_code_graph()
        .unwrap_err();
    assert!(err.to_string().contains("`nodes` must be an array"));

    let wrong_schema = root.join("wrong-schema.json");
    write_json(
        &wrong_schema,
        &json!({"schema_version": 999, "nodes": [], "edges": []}),
    );
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&wrong_schema)
        .read_code_graph()
        .unwrap_err();
    assert!(err.to_string().contains("schema 999"));
    assert!(err.to_string().contains("schema 2"));

    for (index, invalid_schema) in [Value::Null, json!("2")].into_iter().enumerate() {
        let path = root.join(format!("invalid-schema-{index}.json"));
        write_json(
            &path,
            &json!({"schema_version": invalid_schema, "nodes": [], "edges": []}),
        );
        let err = FilesystemArtifactProvider::for_repo(&root)
            .with_code_graph_path(&path)
            .read_code_graph()
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("does not declare a numeric schema"));
    }

    let wrong_map = root.join("wrong-map.json");
    write_json(&wrong_map, &json!({"files": [], "subsystems": {}}));
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_repo_map_path(&wrong_map)
        .read_repo_map()
        .unwrap_err();
    assert_eq!(err.kind(), ArtifactKind::RepoMap);
    assert!(err.to_string().contains("`subsystems` must be an array"));

    for invalid_limit in [0, DEFAULT_ARTIFACT_BYTES + 1, u64::MAX] {
        let err = FilesystemArtifactProvider::for_repo(&root)
            .with_code_graph_path(&wrong_schema)
            .with_max_bytes(invalid_limit)
            .read_code_graph()
            .unwrap_err();
        assert!(err.to_string().contains("byte limit"));
        assert!(err.to_string().contains("invalid"));
    }

    assert_eq!(DEFAULT_ARTIFACT_BYTES, 128 * 1024 * 1024);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_shapes_stay_compatible_but_incomplete_exports_are_refused() {
    let root = scratch("compatibility");
    let graph_path = root.join("graph.json");
    let mut legacy = graph();
    legacy.as_object_mut().unwrap().remove("schema_version");
    write_json(&graph_path, &legacy);
    let provider = FilesystemArtifactProvider::for_repo(&root).with_code_graph_path(&graph_path);
    assert_eq!(provider.read_code_graph().unwrap(), legacy);

    for tier in ["compact", "stub", "future"] {
        let mut incomplete = legacy.clone();
        incomplete["meta"] = json!({"compatibility_export_tier": tier});
        write_json(&graph_path, &incomplete);
        let err = provider.read_code_graph().unwrap_err();
        assert!(err.to_string().contains(tier));
        assert!(err.to_string().contains("incomplete"));
    }

    let mut slim = legacy.clone();
    slim["meta"] = json!({"compatibility_export_tier": "slim"});
    write_json(&graph_path, &slim);
    assert_eq!(provider.read_code_graph().unwrap(), slim);

    let mut disclosed = legacy;
    disclosed["meta"] = json!({"graph_export_incomplete_reason": "edges were capped"});
    write_json(&graph_path, &disclosed);
    let err = provider.read_code_graph().unwrap_err();
    assert!(err.to_string().contains("edges were capped"));

    let map_path = root.join("map.json");
    let minimal_map = json!({"subsystems": []});
    write_json(&map_path, &minimal_map);
    assert_eq!(
        FilesystemArtifactProvider::for_repo(&root)
            .with_repo_map_path(&map_path)
            .read_repo_map()
            .unwrap(),
        minimal_map
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_array_entries_are_refused_instead_of_projecting_as_empty() {
    let provider: &dyn ArtifactProvider = &MemoryProvider {
        graph: json!({"nodes": [null], "edges": [null]}),
        map: json!({"subsystems": [null]}),
    };

    assert!(provider
        .code_graph_payload(&VizOptions::default())
        .unwrap_err()
        .to_string()
        .contains("nodes[0]"));
    assert!(provider
        .repo_map_payload()
        .unwrap_err()
        .to_string()
        .contains("subsystems[0]"));

    let provider: &dyn ArtifactProvider = &MemoryProvider {
        graph: json!({"nodes": [{}], "edges": [{}]}),
        map: json!({"subsystems": [{}]}),
    };
    assert!(provider
        .read_code_graph()
        .unwrap_err()
        .to_string()
        .contains("nodes[0].id"));
    assert!(provider
        .read_repo_map()
        .unwrap_err()
        .to_string()
        .contains("subsystems[0].area"));
}

#[test]
fn modern_maps_must_carry_honest_completeness_metadata() {
    let mut missing_meta = repo_map();
    missing_meta
        .as_object_mut()
        .unwrap()
        .remove("liveness_meta");
    let provider = MemoryProvider {
        graph: graph(),
        map: missing_meta,
    };
    assert!(provider
        .read_repo_map()
        .unwrap_err()
        .to_string()
        .contains("liveness_meta"));

    let mut missing_role_counts = repo_map();
    missing_role_counts["subsystems"][0]
        .as_object_mut()
        .unwrap()
        .remove("role_file_counts");
    let provider = MemoryProvider {
        graph: graph(),
        map: missing_role_counts,
    };
    assert!(provider
        .read_repo_map()
        .unwrap_err()
        .to_string()
        .contains("role_file_counts"));

    let mut missing_role_total = repo_map();
    missing_role_total["subsystems"][0]["role_files"] = json!({"tests": ["src/a_test.rs"]});
    let provider = MemoryProvider {
        graph: graph(),
        map: missing_role_total,
    };
    assert!(provider
        .read_repo_map()
        .unwrap_err()
        .to_string()
        .contains("role_file_counts.tests"));

    let mut missing_dead_meta = repo_map();
    missing_dead_meta["liveness_meta"]
        .as_object_mut()
        .unwrap()
        .remove("dead_symbol");
    let provider = MemoryProvider {
        graph: graph(),
        map: missing_dead_meta,
    };
    assert!(provider
        .read_repo_map()
        .unwrap_err()
        .to_string()
        .contains("liveness_meta.dead_symbol"));

    let mut overflowing = repo_map();
    overflowing["subsystems"][0]["role_files"] = json!({"tests": [], "api": []});
    overflowing["subsystems"][0]["role_file_counts"] = json!({"tests": u64::MAX, "api": u64::MAX});
    let provider = MemoryProvider {
        graph: graph(),
        map: overflowing,
    };
    assert!(provider
        .read_repo_map()
        .unwrap_err()
        .to_string()
        .contains("overflow"));

    let mut dishonest = repo_map();
    dishonest["liveness_meta"]["subsystems"]["total"] = json!(0);
    let provider = MemoryProvider {
        graph: graph(),
        map: dishonest,
    };
    assert!(provider
        .read_repo_map()
        .unwrap_err()
        .to_string()
        .contains("subsystems.total"));
}

#[test]
fn directories_are_refused_before_reading() {
    let root = scratch("directory");
    let directory = root.join("graph.json");
    std::fs::create_dir(&directory).unwrap();
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&directory)
        .read_code_graph()
        .unwrap_err();
    assert!(err.to_string().contains("regular file"));
    std::fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn symlinks_and_fifos_are_refused_before_open() {
    use std::os::unix::fs::symlink;

    let root = scratch("nonregular");
    let target = root.join("target.json");
    write_json(&target, &graph());
    let link = root.join("link.json");
    symlink(&target, &link).unwrap();
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&link)
        .read_code_graph()
        .unwrap_err();
    assert!(err.to_string().contains("regular file"));

    let fifo = root.join("graph.fifo");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo is available on Unix");
    assert!(status.success());
    let started = std::time::Instant::now();
    let err = FilesystemArtifactProvider::for_repo(&root)
        .with_code_graph_path(&fifo)
        .read_code_graph()
        .unwrap_err();
    assert!(err.to_string().contains("regular file"));
    assert!(started.elapsed().as_secs() < 1, "FIFO read blocked");
    std::fs::remove_dir_all(root).ok();
}

struct MemoryProvider {
    graph: Value,
    map: Value,
}

impl ArtifactProvider for MemoryProvider {
    fn load_artifact(
        &self,
        kind: ArtifactKind,
    ) -> Result<Value, devmap_query::host::ArtifactError> {
        Ok(match kind {
            ArtifactKind::CodeGraph => self.graph.clone(),
            ArtifactKind::RepoMap => self.map.clone(),
        })
    }
}

#[test]
fn a_host_can_replace_the_filesystem_provider_without_replacing_consumers() {
    let provider: &dyn ArtifactProvider = &MemoryProvider {
        graph: graph(),
        map: repo_map(),
    };
    assert_eq!(
        provider.code_graph_payload(&VizOptions::default()).unwrap()["counts"]["nodes_total"],
        2
    );
    assert_eq!(
        provider.repo_map_payload().unwrap()["nodes"][0]["id"],
        "src"
    );
}

#[test]
fn replacement_providers_cannot_bypass_validation_through_public_helpers() {
    let provider: &dyn ArtifactProvider = &MemoryProvider {
        graph: json!({"schema_version": 2, "nodes": null, "edges": []}),
        map: json!({"files": [], "subsystems": null}),
    };
    assert!(provider
        .read_code_graph()
        .unwrap_err()
        .to_string()
        .contains("`nodes` must be an array"));
    assert!(provider
        .repo_map_payload()
        .unwrap_err()
        .to_string()
        .contains("`subsystems` must be an array"));
}
