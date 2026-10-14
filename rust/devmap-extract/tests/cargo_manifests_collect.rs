//! The manifest pass reads what each `Cargo.toml` says its crate is — names,
//! `[lib] path`, `[[bin]] path`, `[workspace] members` — and a manifest it
//! cannot read, or one naming a path that is not there, is a recorded gap.
//!
//! Before this, `parse_cargo_crate_names` answered `None` for a virtual
//! workspace and for invalid TOML alike, and the collector dropped both with
//! no row: a broken `Cargo.toml` took its crate's name with it and every
//! `use <name>::…` was filed `External` with nothing saying why.

use std::path::{Path, PathBuf};

use devmap_extract::collect_project_manifests;
use devmap_extract::gomod::{parse_cargo_manifest, CargoManifest};
use devmap_extract::model::DiscoverySkipReason;
use devmap_extract::RustCrateManifest;

fn reason_text(reason: &DiscoverySkipReason) -> String {
    match reason {
        DiscoverySkipReason::Unreadable { reason } => reason.clone(),
        other => format!("{other:?}"),
    }
}

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-cargo-manifests-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn refusal_for(refusals: &[(String, DiscoverySkipReason)], path: &str) -> Option<String> {
    refusals
        .iter()
        .find(|(refused, _)| refused == path)
        .map(|(_, reason)| reason_text(reason))
}

#[test]
fn target_paths_and_names_are_read_relative_to_the_manifest() {
    let parsed = parse_cargo_manifest(
        "crates/engine",
        "[package]\nname = \"engine-core\"\n\n[lib]\nname = \"ecore\"\npath = \"lib/core.rs\"\n\n\
         [[bin]]\nname = \"cli\"\npath = \"../tools/./cli/entry.rs\"\n\n[[bin]]\nname = \"auto\"\n",
    )
    .unwrap();
    assert_eq!(
        parsed,
        CargoManifest {
            package: Some(RustCrateManifest {
                dir: "crates/engine".to_string(),
                names: vec!["ecore".to_string(), "engine_core".to_string()],
                lib_path: Some("crates/engine/lib/core.rs".to_string()),
                bin_paths: vec!["crates/tools/cli/entry.rs".to_string()],
            }),
            workspace_members: Vec::new(),
        }
    );
}

/// A virtual workspace names no crate and is not an error; invalid TOML is.
/// The two used to be one `None`.
#[test]
fn a_virtual_workspace_is_not_a_parse_failure() {
    let virtual_workspace =
        parse_cargo_manifest("", "[workspace]\nmembers = [\"a\", \"crates/*\"]\n").unwrap();
    assert_eq!(virtual_workspace.package, None);
    assert_eq!(virtual_workspace.workspace_members, vec!["a", "crates/*"]);

    for (source, expected) in [
        ("[package\nname = \"x\"\n", "could not be parsed as TOML"),
        (
            "[dependencies]\nserde = \"1\"\n",
            "neither a `[package]` nor a `[workspace]`",
        ),
        (
            "[package]\nversion = \"0.1.0\"\n",
            "`[package]` has no `name`",
        ),
        (
            "[package]\nname = \"x\"\n[lib]\npath = 7\n",
            "`lib.path` is not a string",
        ),
        (
            "[package]\nname = \"x\"\n[lib]\npath = \"../../out.rs\"\n",
            "leaves the repository",
        ),
        (
            "[package]\nname = \"x\"\n[[bin]]\npath = [\"a\"]\n",
            "`bin.path` is not a string",
        ),
        (
            "[workspace]\nmembers = \"a\"\n",
            "`workspace.members` is not an array",
        ),
    ] {
        let error = parse_cargo_manifest("pkg", source).unwrap_err();
        assert!(error.contains(expected), "{source:?}: {error}");
    }
}

#[test]
fn an_unparseable_cargo_toml_is_a_recorded_gap_not_silence() {
    let root = scratch("unparseable");
    write(&root, "broken/Cargo.toml", "[package\nname = \"broken\"\n");
    write(&root, "broken/src/lib.rs", "pub fn f() {}\n");
    write(&root, "virtual/Cargo.toml", "[workspace]\nmembers = []\n");
    write(&root, "good/Cargo.toml", "[package]\nname = \"good-one\"\n");
    write(&root, "good/src/lib.rs", "pub fn g() {}\n");

    let manifests = collect_project_manifests(&root).unwrap();
    let reason = refusal_for(&manifests.refusals, "broken/Cargo.toml")
        .expect("an unparseable Cargo.toml must be a refusal row");
    assert!(
        reason.contains("could not be read as a Cargo manifest")
            && reason.contains("could not be parsed as TOML"),
        "{reason}"
    );
    assert_eq!(
        refusal_for(&manifests.refusals, "virtual/Cargo.toml"),
        None,
        "a virtual workspace is a manifest read cleanly"
    );
    assert!(manifests.examined.contains("virtual/Cargo.toml"));
    let dirs: Vec<&str> = manifests
        .rust_crates
        .iter()
        .map(|krate| krate.dir.as_str())
        .collect();
    assert_eq!(dirs, vec!["good"]);
    assert_eq!(manifests.rust_crates[0].names, vec!["good_one"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn declared_paths_that_do_not_exist_are_recorded_and_the_rest_is_kept() {
    let root = scratch("missing");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"engine\", \"gone\", \"crates/*\", \"nowhere/*\"]\n",
    );
    write(
        &root,
        "engine/Cargo.toml",
        "[package]\nname = \"engine\"\n[lib]\npath = \"lib/core.rs\"\n\
         [[bin]]\nname = \"a\"\npath = \"cli/a.rs\"\n[[bin]]\nname = \"b\"\npath = \"cli/b.rs\"\n",
    );
    write(&root, "engine/lib/core.rs", "pub fn run() {}\n");
    write(&root, "engine/cli/a.rs", "fn main() {}\n");
    write(
        &root,
        "crates/one/Cargo.toml",
        "[package]\nname = \"one\"\n",
    );

    let manifests = collect_project_manifests(&root).unwrap();
    let workspace =
        refusal_for(&manifests.refusals, "Cargo.toml").expect("members with no manifest are a gap");
    assert!(workspace.contains("member `gone`"), "{workspace}");
    assert!(workspace.contains("member `nowhere/*`"), "{workspace}");
    assert!(!workspace.contains("member `engine`"), "{workspace}");
    assert!(!workspace.contains("member `crates/*`"), "{workspace}");

    let engine = refusal_for(&manifests.refusals, "engine/Cargo.toml")
        .expect("a target path that is not a file is a gap");
    assert!(engine.contains("target `engine/cli/b.rs`"), "{engine}");
    assert!(!engine.contains("core.rs"), "{engine}");

    let engine_crate = manifests
        .rust_crates
        .iter()
        .find(|krate| krate.dir == "engine")
        .expect("the package is still read");
    assert_eq!(engine_crate.lib_path.as_deref(), Some("engine/lib/core.rs"));
    assert_eq!(
        engine_crate.bin_paths,
        vec!["engine/cli/a.rs".to_string(), "engine/cli/b.rs".to_string()]
    );
    let _ = std::fs::remove_dir_all(&root);
}
