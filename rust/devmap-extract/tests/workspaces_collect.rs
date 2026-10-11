//! npm / yarn / bun workspaces are collected beside source, and what could not
//! be turned into a mapping is a recorded gap, never silence.

use std::fs;

use devmap_extract::collect_project_manifests;
use devmap_extract::model::DiscoverySkipReason;

fn tree(files: &[(&str, &str)]) -> tempdir_lite::Dir {
    let dir = tempdir_lite::Dir::new();
    for (path, body) in files {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, body).unwrap();
    }
    dir
}

/// A scratch directory without a new dependency.
mod tempdir_lite {
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new() -> Self {
            let unique = format!(
                "devmap-workspaces-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}

fn reasons(manifests: &devmap_extract::ProjectManifests, path: &str) -> Vec<String> {
    manifests
        .refusals
        .iter()
        .filter(|(refused, _)| refused == path)
        .filter_map(|(_, why)| match why {
            DiscoverySkipReason::Unreadable { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn members_are_linked_by_name_to_their_directory() {
    let dir = tree(&[
        ("package.json", r#"{"name":"root","workspaces":["packages/*"]}"#),
        (
            "packages/shared/package.json",
            r#"{"name":"@acme/shared","main":"src/index.ts"}"#,
        ),
    ]);
    let manifests = collect_project_manifests(dir.path()).unwrap();
    assert_eq!(manifests.ts_workspaces.len(), 1);
    assert_eq!(manifests.ts_workspaces[0].name, "@acme/shared");
    assert_eq!(manifests.ts_workspaces[0].dir, "packages/shared");
    assert!(manifests.refusals.is_empty(), "{:?}", manifests.refusals);
}

#[test]
fn a_package_json_that_does_not_parse_is_a_coverage_gap_not_silence() {
    let dir = tree(&[
        ("package.json", r#"{"name":"root","workspaces":["packages/*"]}"#),
        ("packages/broken/package.json", "{ \"name\": "),
    ]);
    let manifests = collect_project_manifests(dir.path()).unwrap();
    let why = reasons(&manifests, "packages/broken/package.json");
    assert_eq!(why.len(), 1, "{:?}", manifests.refusals);
    assert!(why[0].contains("could not be parsed"), "{why:?}");
    assert!(manifests.examined.contains("packages/broken/package.json"));
}

#[test]
fn a_workspace_pattern_naming_no_member_is_a_gap() {
    let dir = tree(&[("package.json", r#"{"workspaces":["apps/*"]}"#)]);
    let manifests = collect_project_manifests(dir.path()).unwrap();
    let why = reasons(&manifests, "package.json");
    assert_eq!(why.len(), 1, "{:?}", manifests.refusals);
    assert!(why[0].contains("apps/*"), "{why:?}");
    assert!(manifests.ts_workspaces.is_empty());
}

#[test]
fn a_package_json_without_workspaces_maps_nothing_and_is_no_gap() {
    let dir = tree(&[("package.json", r#"{"name":"lone"}"#)]);
    let manifests = collect_project_manifests(dir.path()).unwrap();
    assert!(manifests.ts_workspaces.is_empty());
    assert!(manifests.refusals.is_empty(), "{:?}", manifests.refusals);
}
