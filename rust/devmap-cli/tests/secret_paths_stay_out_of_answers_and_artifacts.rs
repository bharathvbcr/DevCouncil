//! The `devmap` binary, end to end, over a repository with credentials in it.
//!
//! The audit's reproduction: `devmap literals AKIA` printed an AWS key from
//! `credentials/`, `devmap ask --evidence 'load prod database password'`
//! printed a `secrets/` password, and both values sat in `devmap.sqlite` and
//! the artifacts beside it, which are copied between worktrees. This runs the
//! same commands and then reads every byte the build left behind.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PLANTED: &[&str] = &[
    "AKIAIOSFODNN7EXAMPLQ",
    "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
    "xoxb-123456789012-abcdefghijkl",
    "hunter2",
];

const SOURCES: &[(&str, &str)] = &[
    (
        "secrets/db.py",
        "DB_PASSWORD = \"hunter2-prod-db\"\n\n\ndef load_prod_database_password():\n    return \"hunter2-prod-db\"\n",
    ),
    (
        "credentials/aws.py",
        "AWS_KEY = \"AKIAIOSFODNN7EXAMPLQ\"\nGH_TOKEN = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\"\n\n\ndef aws_credentials():\n    return \"xoxb-123456789012-abcdefghijkl\"\n",
    ),
    (".env", "DB_PASSWORD=hunter2\nAWS_KEY=AKIAIOSFODNN7EXAMPLQ\n"),
    (
        "deploy/server.key",
        "-----BEGIN PRIVATE KEY-----\nhunter2\n-----END PRIVATE KEY-----\n",
    ),
    (
        "config/settings.py",
        "AWS_KEY = \"AKIAIOSFODNN7EXAMPLQ\"\n",
    ),
    (
        "src/app.py",
        "def load_config():\n    return \"plain-value\"\n",
    ),
];

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    let output = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    assert!(
        output.status.success(),
        "devmap {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-secret-paths-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in SOURCES {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.invalid"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "fixture"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    root.canonicalize().unwrap()
}

#[track_caller]
fn assert_clean(label: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for value in PLANTED {
        assert!(!text.contains(value), "{label} carries {value:?}:\n{text}");
    }
}

/// Every file under `dir` except the planted sources and git's own objects.
fn files_written_by_devmap(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(root).unwrap();
        if relative == Path::new(".git") {
            continue;
        }
        if path.is_dir() {
            files_written_by_devmap(root, &path, out);
        } else if !SOURCES
            .iter()
            .any(|(source, _)| relative == Path::new(source))
        {
            out.push(path);
        }
    }
}

#[test]
fn no_answer_and_no_artifact_carries_a_planted_value() {
    let root = fixture();
    run(&root, &["build"]);
    run(&root, &["manifest"]);

    // `hunter` rather than `hunter2`: an answer echoes its own query. A query
    // matches anywhere in a value, so pieces from inside each value too.
    for query in [
        "AKIA", "ghp_", "xoxb", "hunter", "IOSFODNN", "unter", "mnopqrst", "-1234567",
    ] {
        let text = run(&root, &["literals", query]);
        assert_clean(&format!("literals {query}"), &text.stdout);
        let json = run(&root, &["--json", "literals", query]);
        assert_clean(&format!("--json literals {query}"), &json.stdout);
    }
    let marked = run(&root, &["--json", "literals", "<withheld"]);
    let marked = String::from_utf8_lossy(&marked.stdout);
    assert!(
        marked.contains("secrets/db.py")
            && marked.contains("credentials/aws.py")
            && marked.contains(r#""withheld":"secret_path""#),
        "secret files keep their sites, marked: {marked}"
    );

    for args in [
        vec!["ask", "--evidence", "load prod database password"],
        vec!["--json", "ask", "--evidence", "load prod database password"],
        vec!["--json", "search", "aws_credentials"],
        vec!["--json", "search", "load_prod_database_password"],
        vec!["explore", "load_prod_database_password"],
        vec!["--json", "skeleton", "credentials/aws.py"],
    ] {
        let output = run(&root, &args);
        assert_clean(&format!("devmap {args:?}"), &output.stdout);
    }
    let evidence = run(&root, &["ask", "--evidence", "load prod database password"]);
    let evidence = String::from_utf8_lossy(&evidence.stdout);
    assert!(
        evidence.contains("secrets/db.py") && evidence.contains("withheld: secret_path"),
        "the secret file is located and its source marked withheld:\n{evidence}"
    );

    let mut written = Vec::new();
    files_written_by_devmap(&root, &root, &mut written);
    assert!(
        written.iter().any(|path| path
            .file_name()
            .is_some_and(|name| name == "code_graph.json")),
        "the artifacts were written: {written:?}"
    );
    assert!(
        written
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == "devmap.sqlite")),
        "the store was written: {written:?}"
    );
    for path in &written {
        assert_clean(&path.display().to_string(), &std::fs::read(path).unwrap());
    }
}
