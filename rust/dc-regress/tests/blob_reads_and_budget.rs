//! One analysis reads each blob once, through a bounded number of git
//! processes, and stops when its caller does.
//!
//! The stand-in for `git` here logs every spawn and runs the real one, so what
//! is asserted is what the kernel started, not what it meant to.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dc_regress::blast::{blast_change_bounded, UnattributedReason};
use dc_regress::history::{HistoryRefusal, CONTENT_CAP};
use dc_regress::{
    AffectedTestFile, BlobIdentity, BlobReader, Budget, ChangeSet, ChangeStatus, ChangedRange,
    CodeGraph, ConeEntry, FileChange, GraphSymbol, Unavailable,
};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    /// A `git` that appends its arguments to `log`, then runs the real one.
    counting: PathBuf,
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

/// A repository of `files` small files (`src/f{i}.rs`), committed once.
fn fixture(label: &str, files: usize) -> (Fixture, String) {
    let base = std::env::temp_dir().join(format!(
        "dc-regress-blobs-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    for i in 0..files {
        std::fs::write(
            repo.join(format!("src/f{i}.rs")),
            format!("pub fn f{i}() {{}}\n"),
        )
        .unwrap();
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "base", "--no-gpg-sign"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let log = base.join("spawns.log");
    let counting = base.join("counting-git");
    std::fs::write(
        &counting,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real_git().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&counting, std::fs::Permissions::from_mode(0o755)).unwrap();
    (
        Fixture {
            base,
            repo,
            counting,
            log,
        },
        head,
    )
}

impl Fixture {
    /// The subcommand of each spawn so far.
    fn spawns(&self) -> Vec<String> {
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
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

    fn count(&self, subcommand: &str) -> usize {
        self.spawns().iter().filter(|s| *s == subcommand).count()
    }
}

#[test]
fn a_reader_starts_a_fixed_number_of_processes_however_many_blobs_it_reads() {
    let mut per_size = Vec::new();
    for files in [3usize, 30] {
        let (fx, head) = fixture("fixed", files);
        let reader = BlobReader::new(fx.counting.as_os_str(), &fx.repo);
        for i in 0..files {
            let path = format!("src/f{i}.rs");
            let identity = reader.identity(&head, &path).expect("identity");
            let blob = reader.read(&head, &path).expect("content");
            assert_eq!(
                blob.identity, identity,
                "the content's id is the id the basis check was given"
            );
            assert_eq!(blob.content, format!("pub fn f{i}() {{}}\n"));
            // The same blob again, from the memo of the first.
            assert_eq!(reader.identity(&head, &path).unwrap(), identity);
        }
        let spawns = fx.spawns();
        assert_eq!(
            fx.count("show"),
            0,
            "no read is a `git show` of its own: {spawns:?}"
        );
        assert!(
            fx.count("cat-file") <= 2,
            "one process for identity and one for content: {spawns:?}"
        );
        per_size.push(spawns.len());
    }
    assert_eq!(
        per_size[0], per_size[1],
        "thirty files cost no more processes than three: {per_size:?}"
    );
}

#[test]
fn one_odd_path_is_refused_alone_and_the_next_read_still_answers() {
    let (fx, head) = fixture("odd", 2);
    let reader = BlobReader::new(fx.counting.as_os_str(), &fx.repo);
    assert!(reader.read(&head, "src/f0.rs").is_ok());
    for refused in ["src/new\nline.rs", "src/absent.rs", "src"] {
        let outcome = reader.read(&head, refused);
        assert!(
            matches!(outcome, Err(HistoryRefusal::GitRefused { .. })),
            "{refused:?}: {outcome:?}"
        );
    }
    let again = reader.read(&head, "src/f1.rs").expect("still answering");
    assert_eq!(again.content, "pub fn f1() {}\n");
    assert!(fx.count("cat-file") <= 2, "{:?}", fx.spawns());
}

#[test]
fn a_blob_past_the_cap_is_skipped_without_losing_the_stream() {
    let (fx, head) = fixture("big", 1);
    std::fs::write(fx.repo.join("big.txt"), vec![b'x'; CONTENT_CAP + 1]).unwrap();
    std::fs::write(fx.repo.join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
    git(&fx.repo, &["add", "-A"]);
    git(&fx.repo, &["commit", "-m", "big", "--no-gpg-sign"]);
    let next = git(&fx.repo, &["rev-parse", "HEAD"]);
    assert_ne!(head, next);
    let reader = BlobReader::new(fx.counting.as_os_str(), &fx.repo);
    assert!(matches!(
        reader.read(&next, "big.txt"),
        Err(HistoryRefusal::OutputCapped { .. })
    ));
    assert!(matches!(
        reader.read(&next, "bin.dat"),
        Err(HistoryRefusal::NotText { .. })
    ));
    assert!(
        reader.identity(&next, "big.txt").is_ok(),
        "an oversized blob still has an identity"
    );
    let small = reader
        .read(&next, "src/f0.rs")
        .expect("the stream is in step");
    assert_eq!(small.content, "pub fn f0() {}\n");
}

#[test]
fn a_git_that_cannot_run_is_a_refusal_not_a_hang() {
    let (fx, head) = fixture("gone", 1);
    let reader = BlobReader::new(std::ffi::OsStr::new("/nonexistent/git"), &fx.repo);
    assert!(matches!(
        reader.read(&head, "src/f0.rs"),
        Err(HistoryRefusal::GitUnavailable { .. })
    ));
    assert!(matches!(
        reader.identity(&head, "src/f0.rs"),
        Err(HistoryRefusal::GitUnavailable { .. })
    ));
}

/// A graph that identifies each file through a reader of its own, the way
/// `StoreGraph` does, and holds one symbol per file.
struct SharedGraph {
    blobs: BlobReader,
    head: String,
}

impl CodeGraph for SharedGraph {
    fn symbols_in(&self, file: &str) -> (Vec<GraphSymbol>, BlobIdentity) {
        let basis = self
            .blobs
            .identity(&self.head, file)
            .unwrap_or(BlobIdentity::Unknown);
        (
            vec![GraphSymbol {
                qualified_name: format!("{file}::f"),
                file_path: file.to_string(),
                span_start: 0,
                span_end: 5,
                body_exact: None,
            }],
            basis,
        )
    }
    fn cone(&self, _symbol: &str, _depth: u32) -> (Vec<ConeEntry>, bool) {
        (Vec::new(), false)
    }
    fn impacted(&self, _seeds: &[String], _depth: u32) -> (Vec<ConeEntry>, bool) {
        (Vec::new(), false)
    }
    fn affected_tests(&self, _seeds: &[String], _depth: u32) -> (Vec<AffectedTestFile>, bool) {
        (Vec::new(), false)
    }
    fn resolve_symptom(&self, _symptom: &str) -> Vec<String> {
        Vec::new()
    }
    fn blob_reader(&self) -> Option<&BlobReader> {
        Some(&self.blobs)
    }
}

fn change_of(files: usize) -> ChangeSet {
    ChangeSet {
        files: (0..files)
            .map(|i| FileChange {
                path: format!("src/f{i}.rs"),
                renamed_from: None,
                status: ChangeStatus::Modified,
                ranges: vec![ChangedRange {
                    start_line: 1,
                    end_line: 1,
                    deletion_only: false,
                }],
            })
            .collect(),
        capped: false,
    }
}

#[test]
fn a_blast_reads_each_blob_once_through_the_graphs_own_reader() {
    let files = 25;
    let (fx, head) = fixture("blast", files);
    let graph = SharedGraph {
        blobs: BlobReader::new(fx.counting.as_os_str(), &fx.repo),
        head: head.clone(),
    };
    let report = blast_change_bounded(
        fx.counting.as_os_str(),
        &fx.repo,
        &graph,
        &change_of(files),
        "test",
        &head,
        3,
        &Budget::default(),
    );
    assert_eq!(report.seeds.len(), files, "every file placed: {report:#?}");
    assert_eq!(fx.count("show"), 0, "{:?}", fx.spawns());
    assert!(
        fx.count("cat-file") <= 2,
        "the graph's basis and the blast's content share one pair of processes: {:?}",
        fx.spawns()
    );
    let blob_spawns = fx.count("cat-file") + fx.count("rev-parse");
    assert!(blob_spawns <= 4, "{:?}", fx.spawns());
}

#[test]
fn a_spent_deadline_is_named_not_a_short_list() {
    let (fx, head) = fixture("deadline", 5);
    let graph = SharedGraph {
        blobs: BlobReader::new(fx.counting.as_os_str(), &fx.repo),
        head: head.clone(),
    };
    let report = blast_change_bounded(
        fx.counting.as_os_str(),
        &fx.repo,
        &graph,
        &change_of(5),
        "test",
        &head,
        3,
        &Budget::within(Duration::ZERO),
    );
    assert!(!report.complete, "{report:#?}");
    assert!(
        report.seeds.is_empty(),
        "nothing was read past the deadline"
    );
    assert!(
        report.unavailable.iter().any(|u| matches!(
            u,
            Unavailable::BudgetExhausted { completed: 0, total: 5, reason } if reason.contains("deadline")
        )),
        "{:?}",
        report.unavailable
    );
    assert_eq!(
        fx.count("log"),
        0,
        "no owner lookup starts after the budget is spent: {:?}",
        fx.spawns()
    );
}

#[test]
fn a_cancel_from_the_caller_stops_the_walk_where_it_is() {
    let (fx, head) = fixture("cancel", 8);
    let graph = SharedGraph {
        blobs: BlobReader::new(fx.counting.as_os_str(), &fx.repo),
        head: head.clone(),
    };
    // Cancels on the fourth poll: three files are placed, the fourth is not.
    let polls = Arc::new(AtomicUsize::new(0));
    let seen = polls.clone();
    let budget = Budget::default().with_cancel(move || seen.fetch_add(1, Ordering::SeqCst) >= 3);
    let report = blast_change_bounded(
        fx.counting.as_os_str(),
        &fx.repo,
        &graph,
        &change_of(8),
        "test",
        &head,
        3,
        &budget,
    );
    assert!(!report.complete);
    assert_eq!(report.seeds.len(), 3, "{report:#?}");
    assert!(
        report.unavailable.iter().any(|u| matches!(
            u,
            Unavailable::BudgetExhausted { completed: 3, total: 8, reason } if reason.contains("cancelled")
        )),
        "{:?}",
        report.unavailable
    );
    // Nothing placed past the stop is reported as unattributed either: the
    // files were not examined, which is what the exhaustion says.
    assert!(!report
        .unattributed
        .iter()
        .any(|u| matches!(u.reason, UnattributedReason::ContentUnavailable { .. })));
}
