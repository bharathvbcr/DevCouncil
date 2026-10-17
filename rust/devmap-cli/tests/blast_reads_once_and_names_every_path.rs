//! `devmap blast` against a real index, with the two things the audit of
//! 2026-10-09 found wrong with how it reads git:
//!
//! * every changed file cost a handful of git processes, and the same blob was
//!   read twice (once for the graph's basis, once for the content);
//! * the diff was read in whatever shape the repository's own config asked
//!   for, and one path git had to quote lost the whole change.
//!
//! The first is asserted by putting a `git` that logs its spawns first on
//! `PATH` — what the kernel started, not what it meant to. The second plants
//! each of the diff-prefix settings and the odd names a repository can hold.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    /// A directory holding a `git` that logs its arguments, then runs the
    /// real one.
    bin: PathBuf,
    log: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fixture(label: &str) -> Fixture {
    let base = std::env::temp_dir().join(format!(
        "devmap-blast-reads-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "Ada"]);
    git(&repo, &["config", "user.email", "ada@example.com"]);
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = base.join("spawns.log");
    let script = bin.join("git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real_git().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    Fixture {
        base,
        repo,
        bin,
        log,
    }
}

fn write(repo: &Path, path: &str, content: &str) {
    let full = repo.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message, "--no-gpg-sign"]);
    git(repo, &["rev-parse", "HEAD"])
}

fn devmap(fx: &Fixture, args: &[&str]) -> (bool, String, String) {
    let path = format!(
        "{}:{}",
        fx.bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(args)
        .arg("--root")
        .arg(&fx.repo)
        .env("PATH", path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("devmap runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn build(fx: &Fixture) {
    let (ok, out, err) = devmap(fx, &["build", "--full"]);
    assert!(ok, "build failed: {out}{err}");
}

fn blast(fx: &Fixture, since: &str) -> serde_json::Value {
    let (ok, out, err) = devmap(fx, &["--json", "blast", "--since", since]);
    assert!(ok, "blast failed: {out}{err}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("blast must emit JSON: {e}\n{out}"))
}

fn changed_paths(report: &serde_json::Value) -> Vec<String> {
    let mut paths: Vec<String> = report["changed_files"]
        .as_array()
        .unwrap_or_else(|| panic!("`changed_files` is an array in {report:#}"))
        .iter()
        .filter_map(|file| file["path"].as_str())
        .map(str::to_string)
        .collect();
    paths.sort();
    paths
}

/// The subcommand of each spawn logged since `mark`.
fn subcommands(fx: &Fixture) -> Vec<String> {
    let log = std::fs::read_to_string(&fx.log).unwrap_or_default();
    log.lines()
        .map(|line| {
            let mut words = line.split(' ');
            while let Some(word) = words.next() {
                if word == "-c" || word == "-C" {
                    words.next();
                } else if !word.starts_with('-') {
                    return word.to_string();
                }
            }
            String::new()
        })
        .collect()
}

/// `files` Python modules, each with one function, then each edited.
fn corpus(label: &str, files: usize) -> (Fixture, String) {
    let fx = fixture(label);
    for i in 0..files {
        write(
            &fx.repo,
            &format!("pkg/m{i}.py"),
            &format!("def f{i}():\n    return {i}\n"),
        );
    }
    let base = commit(&fx.repo, "base");
    for i in 0..files {
        write(
            &fx.repo,
            &format!("pkg/m{i}.py"),
            &format!("def f{i}():\n    return {i} + 1\n"),
        );
    }
    commit(&fx.repo, "edit every file");
    build(&fx);
    (fx, base)
}

#[test]
fn every_blob_is_read_through_a_fixed_number_of_git_processes() {
    let mut blob_spawns = Vec::new();
    for files in [4usize, 24] {
        let (fx, base) = corpus("spawns", files);
        // Only what the blast itself starts.
        let _ = std::fs::remove_file(&fx.log);
        let report = blast(&fx, &base);
        let seeds = report["seeds"].as_array().expect("seeds");
        assert!(
            seeds.len() >= files,
            "every edited function is a seed, so every blob was read: {} of {files}\n{report:#}",
            seeds.len()
        );
        let spawned = subcommands(&fx);
        let count = |name: &str| spawned.iter().filter(|s| *s == name).count();
        assert_eq!(count("show"), 0, "no per-file `git show`: {spawned:?}");
        blob_spawns.push(count("show") + count("cat-file") + count("rev-parse"));
    }
    assert!(
        blob_spawns.iter().all(|n| *n <= 8),
        "blob reads start a bounded number of processes: {blob_spawns:?}"
    );
    assert_eq!(
        blob_spawns[0], blob_spawns[1],
        "six times the files, no more processes: {blob_spawns:?}"
    );
}

/// The diff's prefixes are the repository's to configure, and none of them
/// may change which file a path names. `a/` and `b/` are directories here, so
/// a reader that strips what it thinks is a prefix takes a real directory
/// name with it.
#[test]
fn the_diff_prefix_settings_do_not_change_which_files_changed() {
    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.srcPrefix", "SRC/"),
        ("diff.dstPrefix", "DST/"),
        ("diff.mnemonicPrefix", "true"),
    ] {
        let fx = fixture("prefix");
        let paths = ["a/lib.py", "b/lib.py", "pkg/lib.py"];
        for path in paths {
            write(&fx.repo, path, "def f():\n    return 1\n");
        }
        let base = commit(&fx.repo, "base");
        for path in paths {
            write(&fx.repo, path, "def f():\n    return 2\n");
        }
        commit(&fx.repo, "edit");
        build(&fx);
        git(&fx.repo, &["config", key, value]);

        let report = blast(&fx, &base);
        assert_eq!(
            changed_paths(&report),
            paths,
            "{key}={value} must not rename the files: {report:#}"
        );
        let renamed = report["changed_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| !file["renamed_from"].is_null());
        assert!(
            !renamed,
            "{key}={value} made a file look renamed: {report:#}"
        );
        assert_eq!(
            report["seeds"].as_array().unwrap().len(),
            3,
            "{key}={value}: each file's function is seeded: {report:#}"
        );
    }
}

/// One file whose name git has to quote used to blind the whole change.
#[test]
fn a_path_git_quotes_does_not_hide_the_files_beside_it() {
    let fx = fixture("odd");
    let odd = [
        "src/nl\nname.py",
        "src/caf\u{e9}.py",
        "src/quo\"te.py",
        "src/back\\slash.py",
        "src/tab\there.py",
        "src/sp ace.py",
        "src/trailing .py",
    ];
    write(&fx.repo, "src/ok.py", "def ok():\n    return 1\n");
    for (i, path) in odd.iter().enumerate() {
        write(&fx.repo, path, &format!("def g{i}():\n    return 1\n"));
    }
    let base = commit(&fx.repo, "base");
    write(&fx.repo, "src/ok.py", "def ok():\n    return 2\n");
    for (i, path) in odd.iter().enumerate() {
        write(&fx.repo, path, &format!("def g{i}():\n    return 2\n"));
    }
    commit(&fx.repo, "edit");
    build(&fx);

    let report = blast(&fx, &base);
    let mut expected: Vec<String> = odd.iter().map(|p| p.to_string()).collect();
    expected.push("src/ok.py".to_string());
    expected.sort();
    assert_eq!(
        changed_paths(&report),
        expected,
        "every changed file is named, quoted or not: {report:#}"
    );
    assert!(
        !report["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["kind"] == "change_unreadable"),
        "{report:#}"
    );
    let seeds: Vec<&str> = report["seeds"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["qualified_name"].as_str())
        .collect();
    assert!(
        seeds.iter().any(|s| s.ends_with("::ok")),
        "the plain file still seeds the walk: {seeds:?}"
    );
}
