//! The manifest pass reads Swift targets from `Package.swift` and an XcodeGen
//! `project.yml`, and a manifest naming targets it cannot follow is a recorded
//! gap, never an empty answer: the files it governs fall back to the path
//! heuristic and the inventory says that happened.

use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-swift-targets-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".git/info")).unwrap();
    root.canonicalize().unwrap()
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn package_and_xcodegen_targets_are_collected_with_their_folders() {
    let root = scratch("collect");
    write(
        &root,
        "ios/project.yml",
        "targets:\n  App:\n    sources: [App, Shared]\n  Widget:\n    sources: [Widget, Shared]\n",
    );
    write(
        &root,
        "ios/Kit/Package.swift",
        "let p = Package(name: \"Kit\", targets: [.target(name: \"Kit\", path: \"Lib\")])\n",
    );
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    let find = |name: &str| {
        manifests
            .swift_targets
            .iter()
            .find(|target| target.name == name)
            .unwrap_or_else(|| panic!("{name} in {:?}", manifests.swift_targets))
    };
    assert_eq!(find("App").paths, vec!["ios/App", "ios/Shared"]);
    assert_eq!(find("Widget").paths, vec!["ios/Widget", "ios/Shared"]);
    assert_eq!(find("Kit").paths, vec!["ios/Kit/Lib"]);
    assert!(find("Kit").package && !find("App").package);
    assert!(manifests.refusals.is_empty(), "{:?}", manifests.refusals);
}

#[test]
fn a_manifest_naming_targets_it_cannot_follow_is_a_recorded_gap() {
    let root = scratch("gap");
    write(
        &root,
        "Package.swift",
        "let p = Package(name: \"P\", targets: [.target(name: coreName)])\n",
    );
    write(
        &root,
        "app/project.yml",
        "targets:\n  A:\n    sources:\n      - name: X\n        path: Y\n",
    );
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    let refused: Vec<&str> = manifests
        .refusals
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    assert!(refused.contains(&"Package.swift"), "{refused:?}");
    assert!(refused.contains(&"app/project.yml"), "{refused:?}");
    assert!(manifests.swift_targets.is_empty());
}
