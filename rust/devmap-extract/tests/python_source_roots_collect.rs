//! The manifest pass reads the Python source roots a project declares, and a
//! manifest it cannot read or whose root is not a directory is a recorded gap
//! — never an empty answer that reads as "this project declares no roots".

use std::path::{Path, PathBuf};

use devmap_extract::collect_project_manifests;
use devmap_extract::model::DiscoverySkipReason;

fn reason_text(reason: &DiscoverySkipReason) -> String {
    match reason {
        DiscoverySkipReason::Unreadable { reason } => reason.clone(),
        other => format!("{other:?}"),
    }
}

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-python-roots-{tag}-{}-{}",
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

#[test]
fn each_manifest_governs_its_own_directory() {
    let root = scratch("governs");
    write(
        &root,
        "pyproject.toml",
        "[tool.pytest.ini_options]\npythonpath = [\"lib\"]\n",
    );
    write(&root, "lib/pkg/__init__.py", "");
    write(
        &root,
        "svc/pyproject.toml",
        "[tool.hatch.build.targets.wheel]\npackages = [\"python/svcpkg\"]\n",
    );
    write(&root, "svc/python/svcpkg/__init__.py", "");
    write(
        &root,
        "legacy/setup.cfg",
        "[options]\npackage_dir =\n    = src\n",
    );
    write(&root, "legacy/src/old/__init__.py", "");

    let manifests = collect_project_manifests(&root).unwrap();
    let found: Vec<(&str, Vec<&str>)> = manifests
        .python_roots
        .iter()
        .map(|set| {
            (
                set.dir.as_str(),
                set.roots.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![
            ("", vec!["lib"]),
            ("legacy", vec!["legacy/src"]),
            ("svc", vec!["svc/python"]),
        ]
    );
    assert!(
        manifests.refusals.is_empty(),
        "nothing to refuse: {:?}",
        manifests.refusals
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_manifest_that_does_not_parse_is_a_recorded_gap() {
    let root = scratch("unparsed");
    write(&root, "pyproject.toml", "[tool.ruff\nsrc = [\n");
    write(&root, "pkg/setup.cfg", "name = outside_any_section\n");

    let manifests = collect_project_manifests(&root).unwrap();
    assert!(manifests.python_roots.is_empty());
    let refused: Vec<(&str, String)> = manifests
        .refusals
        .iter()
        .map(|(path, reason)| (path.as_str(), reason_text(reason)))
        .collect();
    assert_eq!(refused.len(), 2, "both are gaps: {refused:?}");
    assert_eq!(refused[0].0, "pkg/setup.cfg");
    assert!(refused[0].1.contains("precedes any section"), "{refused:?}");
    assert_eq!(refused[1].0, "pyproject.toml");
    assert!(refused[1].1.contains("TOML"), "{refused:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_root_that_is_not_a_directory_is_a_gap_and_is_not_probed() {
    let root = scratch("missing");
    write(
        &root,
        "pyproject.toml",
        "[tool.ruff]\nsrc = [\"python\", \"ghost\", \"../outside\"]\n",
    );
    write(&root, "python/pkg/__init__.py", "");

    let manifests = collect_project_manifests(&root).unwrap();
    // `../outside` leaves the repository: the whole manifest is unreadable.
    assert!(manifests.python_roots.is_empty(), "{manifests:?}");
    assert_eq!(manifests.refusals.len(), 1);
    assert!(
        reason_text(&manifests.refusals[0].1).contains("leaves the repository"),
        "{:?}",
        manifests.refusals
    );

    write(
        &root,
        "pyproject.toml",
        "[tool.ruff]\nsrc = [\"python\", \"ghost\"]\n",
    );
    let manifests = collect_project_manifests(&root).unwrap();
    assert_eq!(manifests.python_roots.len(), 1);
    assert_eq!(manifests.python_roots[0].roots, ["python"]);
    assert_eq!(manifests.refusals.len(), 1);
    let reason = reason_text(&manifests.refusals[0].1);
    assert!(
        reason.contains("not directories") && reason.contains("ghost"),
        "{reason}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
