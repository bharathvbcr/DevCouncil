//! Git's global excludes file (`core.excludesFile`) is a rule source the walk
//! reads, so the watcher reads it too.
//!
//! Its own test binary because it sets `GIT_CONFIG_GLOBAL`, which is process
//! state: in a shared binary it would race every other test that walks a tree.

use std::path::Path;

use devmap_extract::{collect_sources_with_report, ignore_rule_files, is_gitignored};

#[test]
fn a_global_excludes_file_excludes_alike_for_the_walk_and_the_watcher() {
    let base = std::env::temp_dir().join(format!(
        "devmap-global-excludes-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(root.join(".git/info")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let base = base.canonicalize().unwrap();
    let root = root.canonicalize().unwrap();
    let excludes = base.join("global-excludes");
    std::fs::write(&excludes, "x.ts\n").unwrap();
    let config = base.join("gitconfig");
    std::fs::write(
        &config,
        format!("[core]\n\texcludesFile = {}\n", excludes.display()),
    )
    .unwrap();
    std::fs::write(root.join("src/x.ts"), "export const x = 1;\n").unwrap();
    std::fs::write(root.join("src/y.ts"), "export const y = 1;\n").unwrap();
    // This binary holds one test, so no other thread reads the environment
    // while it is written.
    std::env::set_var("GIT_CONFIG_GLOBAL", &config);

    let walked: Vec<String> = collect_sources_with_report(&root)
        .unwrap()
        .0
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    let excluded = |relative: &str| is_gitignored(&root, &root.join(relative), false).unwrap();
    assert!(
        !walked.contains(&"src/x.ts".to_string()),
        "precondition: the walk honours core.excludesFile: {walked:?}"
    );
    assert!(
        excluded("src/x.ts"),
        "the watcher must exclude what the global excludes file names, as the walk does"
    );
    assert!(walked.contains(&"src/y.ts".to_string()));
    assert!(!excluded("src/y.ts"));
    assert!(
        ignore_rule_files(&root, &root.join("src/x.ts"), false)
            .unwrap()
            .contains(&excludes),
        "and stamps it, so an edit to it — outside the watched tree — re-evaluates"
    );
    assert!(devmap_extract::is_ignore_rule_file(Path::new(&excludes)));

    let _ = std::fs::remove_dir_all(&base);
}
