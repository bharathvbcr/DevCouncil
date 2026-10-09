//! Git's global excludes file binds the daemon as it binds the build.
//!
//! Its own test binary because it sets `GIT_CONFIG_GLOBAL`, process state the
//! `devmap` subprocesses inherit and every other test in a shared binary would
//! race.

mod support_ignored;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_named_in_core_excludes_file_is_not_enqueued_and_status_stays_fresh() {
    let root = support_ignored::scratch("global-excludes");
    let config_dir = root.with_file_name(format!(
        "{}-gitconfig",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&config_dir).unwrap();
    let excludes = config_dir.join("ignore");
    std::fs::write(&excludes, "x.ts\n").unwrap();
    let config = config_dir.join("config");
    std::fs::write(
        &config,
        format!("[core]\n\texcludesFile = {}\n", excludes.display()),
    )
    .unwrap();
    // One test in this binary: nothing else reads the environment meanwhile.
    std::env::set_var("GIT_CONFIG_GLOBAL", &config);
    std::fs::write(root.join("src/keep.ts"), "export const keep = 1;\n").unwrap();
    support_ignored::excluded_path_stays_out(&root, "src/x.ts", "src/y.ts").await;
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&config_dir);
}
