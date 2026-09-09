//! First-run discovery must name the same store the writer actually uses.
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "devmap-state-contract-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("exclusive disposable fixture");
        fs::create_dir_all(root.join("repo/src")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("repo/src/a.py"), "def indexed():\n    return 1\n").unwrap();
        let output = Command::new("git")
            .args(["-c", "init.templateDir=", "init", "-q"])
            .arg(root.join("repo"))
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self(root)
    }

    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }

    fn run(&self, cwd: &Path, args: &[&str], home: Option<&Path>) -> serde_json::Value {
        self.run_expect(cwd, args, home, true)
    }

    fn run_expect(
        &self,
        cwd: &Path,
        args: &[&str],
        home: Option<&Path>,
        success: bool,
    ) -> serde_json::Value {
        // File-backed pipes cannot deadlock a child with a verbose diagnostic.
        let stdout = self.0.join("stdout");
        let stderr = self.0.join("stderr");
        let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
        command
            .current_dir(cwd)
            .arg("--json")
            .args(args)
            .env_remove("DEVMAP_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("DEVMAP_AUTOSPAWN", "0")
            .env("TOKIO_WORKER_THREADS", "2")
            .env("RAYON_NUM_THREADS", "2")
            .stdout(Stdio::from(File::create(&stdout).unwrap()))
            .stderr(Stdio::from(File::create(&stderr).unwrap()));
        if let Some(home) = home {
            command.env("DEVMAP_HOME", home);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("devmap {args:?} exceeded 30 seconds");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let output = fs::read_to_string(stdout).unwrap();
        assert_eq!(
            status.success(),
            success,
            "devmap {args:?}: {output}\n{}",
            fs::read_to_string(stderr).unwrap()
        );
        serde_json::from_str(&output).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove disposable fixture");
    }
}

#[test]
fn relative_repository_paths_do_not_duplicate_the_repository_directory() {
    let fixture = Fixture::new();
    fixture.run(&fixture.0, &["build", "repo", "--manifest"], None);
    let report = fixture.run(&fixture.0, &["paths", "repo"], None);
    let expected = fixture.repo().join(".devmap/codeintel/devmap.sqlite");
    assert_eq!(report["db_path"].as_str(), expected.to_str());
    assert_eq!(report["store_exists"], true);
}

#[test]
fn explicit_relative_database_paths_stay_relative_to_the_invoking_directory() {
    let fixture = Fixture::new();
    fixture.run(
        &fixture.0,
        &["--db", "custom.sqlite", "build", "repo"],
        None,
    );
    let report = fixture.run(
        &fixture.0,
        &["--db", "custom.sqlite", "paths", "repo"],
        None,
    );
    assert_eq!(
        report["db_path"].as_str(),
        fixture.0.join("custom.sqlite").to_str()
    );
    assert_eq!(report["store_exists"], true);
}

#[test]
fn nested_discovery_and_status_agree_without_creating_a_second_store() {
    let fixture = Fixture::new();
    fixture.run(&fixture.repo(), &["build", "--manifest"], None);
    let nested = fixture.repo().join("src");
    let status = fixture.run(&nested, &["status"], None);
    let paths = fixture.run(&nested, &["paths"], None);
    assert_eq!(status["query_ready"], true);
    assert_eq!(paths["db_path"], status["db_path"]);
    assert_eq!(paths["root"].as_str(), fixture.repo().to_str());
    assert!(!nested.join(".devmap").exists());
}

#[test]
fn default_build_from_a_subdirectory_bootstraps_the_same_store_queries_use() {
    let fixture = Fixture::new();
    let nested = fixture.repo().join("src");
    fixture.run(&nested, &["build", "--manifest"], None);
    let status = fixture.run(&nested, &["status"], None);
    assert_eq!(status["query_ready"], true);
    assert!(fixture.repo().join(".devmap/repo_map.json").is_file());
    assert!(!nested.join(".devmap").exists());
}

#[test]
fn an_explicit_subtree_is_still_an_explicit_scope() {
    let fixture = Fixture::new();
    let nested = fixture.repo().join("src");
    fixture.run(&nested, &["build", "."], None);
    let report = fixture.run(&nested, &["paths", "."], None);
    assert_eq!(report["root"].as_str(), nested.to_str());
    assert_eq!(report["store_exists"], true);
    assert!(!fixture.repo().join(".devmap").exists());
}

#[test]
fn relative_home_override_is_repository_relative_only_once() {
    let fixture = Fixture::new();
    let home = Path::new("state");
    fixture.run(&fixture.0, &["build", "repo", "--manifest"], Some(home));
    let report = fixture.run(&fixture.0, &["paths", "repo"], Some(home));
    assert_eq!(
        report["db_path"].as_str(),
        fixture
            .repo()
            .join("state/codeintel/devmap.sqlite")
            .to_str()
    );
    assert_eq!(report["store_exists"], true);
}

#[test]
fn map_html_uses_the_same_standalone_legacy_and_override_layout_as_build() {
    for layout in [".devmap", ".devcouncil", "custom-state"] {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.repo().join(layout)).unwrap();
        let home = (layout == "custom-state").then(|| Path::new(layout));
        fixture.run(&fixture.repo(), &["build", "--manifest"], home);
        let report = fixture.run(&fixture.repo(), &["map-html"], home);
        assert_eq!(report["written"], true);
        assert!(fixture.repo().join(layout).join("map.html").is_file());
        if layout != ".devcouncil" {
            assert!(!fixture.repo().join(".devcouncil").exists());
        }
    }
}

#[test]
fn map_html_refuses_incomplete_and_oversized_inputs_before_writing_a_page() {
    let fixture = Fixture::new();
    let input = fixture.repo().join("invalid.json");
    for body in ["{}", "[]", "null", "{\"files\":[]}"] {
        fs::write(&input, body).unwrap();
        let result = fixture.run_expect(
            &fixture.repo(),
            &["map-html", "--input", "invalid.json"],
            None,
            false,
        );
        assert!(
            result["error"].as_str().unwrap().contains("invalid shape"),
            "{result}"
        );
        assert!(!fixture.repo().join(".devmap/map.html").exists());
    }
    File::create(&input)
        .unwrap()
        .set_len(devmap_query::host::DEFAULT_ARTIFACT_BYTES + 1)
        .unwrap();
    let result = fixture.run_expect(
        &fixture.repo(),
        &["map-html", "--input", "invalid.json"],
        None,
        false,
    );
    assert!(
        result["error"].as_str().unwrap().contains("limit"),
        "{result}"
    );
    assert!(!fixture.repo().join(".devmap/map.html").exists());
}

#[cfg(unix)]
#[test]
fn map_html_refuses_symlinks_instead_of_silently_loading_another_file() {
    let fixture = Fixture::new();
    fixture.run(&fixture.repo(), &["build", "--manifest"], None);
    std::os::unix::fs::symlink(
        fixture.repo().join(".devmap/repo_map.json"),
        fixture.repo().join("linked.json"),
    )
    .unwrap();
    let result = fixture.run_expect(
        &fixture.repo(),
        &["map-html", "--input", "linked.json"],
        None,
        false,
    );
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("not a regular file"),
        "{result}"
    );
    assert!(!fixture.repo().join(".devmap/map.html").exists());
}

#[test]
fn a_new_linked_worktree_is_truthfully_missing_until_bootstrapped_locally() {
    let fixture = Fixture::new();
    for args in [
        vec!["add", "src/a.py"],
        vec!["commit", "-qm", "fixture"],
        vec!["worktree", "add", "--detach", "../linked"],
    ] {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.hooksPath=",
            ])
            .args(args)
            .current_dir(fixture.repo())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fixture.run(&fixture.repo(), &["build", "--manifest"], None);
    let linked = fixture.0.join("linked");
    let nested = linked.join("src");
    assert!(linked.join(".git").is_file());
    let missing = fixture.run(&nested, &["status"], None);
    assert_eq!(missing["query_ready"], false);
    let paths = fixture.run(&nested, &["paths"], None);
    assert_eq!(paths["root"].as_str(), linked.to_str());
    assert_eq!(paths["db_path"], missing["db_path"]);
    assert_eq!(paths["store_exists"], false);
    assert!(
        !linked.join(".devmap").exists(),
        "read-only discovery must not bootstrap"
    );
    fixture.run(&nested, &["build", "--manifest"], None);
    assert_eq!(fixture.run(&nested, &["status"], None)["query_ready"], true);
    assert!(linked.join(".devmap/repo_map.json").is_file());
    assert!(!nested.join(".devmap").exists());
}

#[test]
fn a_database_only_build_can_be_completed_or_repaired_with_manifest() {
    let fixture = Fixture::new();
    fixture.run(&fixture.repo(), &["build"], None);
    let map = fixture.repo().join(".devmap/repo_map.json");
    assert!(
        !map.exists(),
        "plain build keeps its database-only contract"
    );
    for _ in 0..2 {
        fixture.run(&fixture.repo().join("src"), &["build", "--manifest"], None);
        assert!(map.is_file());
        assert!(fixture
            .repo()
            .join(".devmap/graph/code_graph.json")
            .is_file());
        fixture.run(&fixture.repo().join("src"), &["map-html"], None);
        fs::remove_file(&map).unwrap();
    }
}

#[test]
fn explicit_map_paths_stay_repository_relative_and_missing_roots_are_refused() {
    let fixture = Fixture::new();
    fs::write(fixture.repo().join("input.json"), "{\"subsystems\":[]}").unwrap();
    fixture.run(
        &fixture.0,
        &[
            "map-html",
            "repo",
            "--input",
            "input.json",
            "--output",
            "preview.html",
        ],
        None,
    );
    assert!(fixture.repo().join("preview.html").is_file());
    let result = fixture.run_expect(
        &fixture.0,
        &[
            "map-html",
            "missing",
            "--input",
            fixture.repo().join("input.json").to_str().unwrap(),
            "--output",
            "preview.html",
        ],
        None,
        false,
    );
    assert!(
        result["error"].as_str().unwrap().contains("missing"),
        "{result}"
    );
    assert!(!fixture.0.join("missing").exists());
}

#[test]
fn omitted_roots_outside_git_still_use_the_invoking_directory() {
    let fixture = Fixture::new();
    let plain = fixture.0.join("plain");
    fs::create_dir(&plain).unwrap();
    fs::write(plain.join("a.py"), "def indexed():\n    return 1\n").unwrap();
    fixture.run(&plain, &["build", "--manifest"], None);
    let paths = fixture.run(&plain, &["paths"], None);
    assert_eq!(paths["root"].as_str(), plain.to_str());
    assert_eq!(paths["store_exists"], true);
    assert_eq!(fixture.run(&plain, &["status"], None)["query_ready"], true);
}
