//! End-to-end against real `git`. Everything else in this crate is unit-tested
//! against hand-built inputs; this is the only place that proves the argv,
//! the porcelain parsing and the blob resolution actually agree with git.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use dc_regress::suspects::suspects;
use dc_regress::{BlobIdentity, CodeGraph, ConeEntry, GraphSymbol};

/// Unique temp roots. Not a timestamp: on macOS two tests entering this within
/// the same nanosecond tick get the same name, and `cargo test` runs them in
/// parallel by default. Process id plus an atomic sequence cannot collide
/// within a run, and the pid separates runs.
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

fn temp_root(label: &str) -> PathBuf {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root =
        std::env::temp_dir().join(format!("dc-regress-{label}-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(label: &str) -> PathBuf {
    let root = temp_root(label);
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Ada"]);
    git(&root, &["config", "user.email", "ada@example.com"]);
    root
}

fn write(repo: &Path, path: &str, body: &str) {
    let full = repo.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(full, body).expect("write");
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message, "--no-gpg-sign"]);
    git(repo, &["rev-parse", "HEAD"])
}

/// A graph built by hand. The real one comes from the store; this is here so
/// the analysis can be driven over shapes a fixture repository can produce
/// without also standing up an index.
struct FakeGraph {
    symbols: Vec<GraphSymbol>,
    cone: Vec<ConeEntry>,
    basis: BlobIdentity,
    resolves_to: Vec<String>,
}

impl CodeGraph for FakeGraph {
    fn symbols_in(&self, file: &str) -> (Vec<GraphSymbol>, BlobIdentity) {
        (
            self.symbols
                .iter()
                .filter(|s| s.file_path == file)
                .cloned()
                .collect(),
            self.basis.clone(),
        )
    }
    fn cone(&self, _symbol: &str, _depth: u32) -> (Vec<ConeEntry>, bool) {
        (self.cone.clone(), false)
    }
    fn resolve_symptom(&self, _symptom: &str) -> Vec<String> {
        self.resolves_to.clone()
    }
}

const V1: &str = "pub fn helper() -> u32 {\n    1\n}\n\npub fn unrelated() -> u32 {\n    99\n}\n";

/// `helper` gains a bug; `unrelated` is reformatted in a later commit.
const V2: &str = "pub fn helper() -> u32 {\n    2\n}\n\npub fn unrelated() -> u32 {\n    99\n}\n";

const V3: &str =
    "pub fn helper() -> u32 {\n    2\n}\n\npub fn unrelated() -> u32 {\n\n    99\n\n}\n";

fn spans_for(source: &str) -> Vec<GraphSymbol> {
    let helper_start = source.find("pub fn helper").unwrap();
    let helper_end = source[helper_start..].find("}\n").unwrap() + helper_start + 1;
    let unrelated_start = source.find("pub fn unrelated").unwrap();
    vec![
        GraphSymbol {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            span_start: helper_start,
            span_end: helper_end,
            body_exact: None,
        },
        GraphSymbol {
            qualified_name: "src/lib.rs::unrelated".into(),
            file_path: "src/lib.rs".into(),
            span_start: unrelated_start,
            span_end: source.len(),
            body_exact: None,
        },
    ]
}

#[test]
fn the_commit_that_changed_the_cone_symbol_is_the_top_suspect() {
    let repo = init_repo("top-suspect");
    write(&repo, "src/lib.rs", V1);
    let base = commit(&repo, "v1");
    write(&repo, "src/lib.rs", V2);
    let bug = commit(&repo, "v2: change helper");
    write(&repo, "src/lib.rs", V3);
    let head = commit(&repo, "v3: reformat unrelated");

    let head_blob = git(&repo, &["rev-parse", &format!("{head}:src/lib.rs")]);
    let graph = FakeGraph {
        symbols: spans_for(V3),
        // Only `helper` is in the cone: the symptom is downstream of it.
        cone: vec![ConeEntry {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            distance: 1,
        }],
        basis: BlobIdentity::Blob(head_blob),
        resolves_to: vec!["src/lib.rs::helper".into()],
    };

    let report = suspects(&repo, &graph, "helper", &base, &head, 3);
    assert!(
        report.complete,
        "nothing should have been refused: {:?}",
        report.unavailable
    );
    assert!(!report.suspects.is_empty(), "the bug commit must be found");
    assert_eq!(
        report.suspects[0].commit, bug,
        "the commit that changed `helper`'s body is the suspect; the later \
         reformat of `unrelated` is not even in the cone"
    );
    assert!(
        report.suspects.iter().all(|s| s.commit != head),
        "a commit that touched no cone symbol must not appear at all"
    );
}

#[test]
fn a_commit_outside_the_window_is_not_a_suspect() {
    let repo = init_repo("window");
    write(&repo, "src/lib.rs", V1);
    let base = commit(&repo, "v1");
    write(&repo, "src/lib.rs", V2);
    let bug = commit(&repo, "v2");

    let head_blob = git(&repo, &["rev-parse", &format!("{bug}:src/lib.rs")]);
    let graph = FakeGraph {
        symbols: spans_for(V2),
        cone: vec![ConeEntry {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            distance: 0,
        }],
        basis: BlobIdentity::Blob(head_blob),
        resolves_to: vec!["src/lib.rs::helper".into()],
    };

    // An empty window: `bug..bug` contains nothing.
    let report = suspects(&repo, &graph, "helper", &bug, &bug, 3);
    assert!(report.suspects.is_empty());
    assert!(
        report.complete,
        "an empty window is a complete answer — there genuinely is nothing in \
         it — and must not be dressed up as a failure: {:?}",
        report.unavailable
    );
    let _ = base;
}

/// The heart of the crate, end to end. Spans taken against one revision must
/// never be read against another's lines.
#[test]
fn spans_from_the_wrong_revision_are_refused_against_a_real_repository() {
    let repo = init_repo("mismatch");
    write(&repo, "src/lib.rs", V1);
    let base = commit(&repo, "v1");
    write(&repo, "src/lib.rs", V3);
    let head = commit(&repo, "v3");

    let stale_blob = git(&repo, &["rev-parse", &format!("{base}:src/lib.rs")]);
    let graph = FakeGraph {
        symbols: spans_for(V1),
        cone: vec![ConeEntry {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            distance: 0,
        }],
        // The graph was built at `base`; the analysis runs at `head`.
        basis: BlobIdentity::Blob(stale_blob),
        resolves_to: vec!["src/lib.rs::helper".into()],
    };

    let report = suspects(&repo, &graph, "helper", &base, &head, 3);
    assert!(
        !report.complete,
        "a stale index must not silently produce an answer"
    );
    assert!(
        report
            .unavailable
            .iter()
            .any(|u| matches!(u, dc_regress::Unavailable::BlobMismatch { .. })),
        "the refusal must name the mismatch, not some generic failure: {:?}",
        report.unavailable
    );
    assert!(report.suspects.is_empty());
}

#[test]
fn a_symptom_the_graph_does_not_know_is_named_as_such() {
    let repo = init_repo("unknown-symptom");
    write(&repo, "src/lib.rs", V1);
    let base = commit(&repo, "v1");
    let graph = FakeGraph {
        symbols: Vec::new(),
        cone: Vec::new(),
        basis: BlobIdentity::Unknown,
        resolves_to: Vec::new(),
    };
    let report = suspects(&repo, &graph, "nope", &base, &base, 3);
    assert!(!report.complete);
    assert!(matches!(
        report.unavailable[0],
        dc_regress::Unavailable::SymptomNotFound { .. }
    ));
}

/// A repository that is not one. The analysis must say git refused, not return
/// an empty list that reads as "no suspects".
#[test]
fn a_directory_that_is_not_a_repository_is_a_named_failure() {
    let root = temp_root("not-a-repo");
    let graph = FakeGraph {
        symbols: Vec::new(),
        cone: vec![ConeEntry {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            distance: 0,
        }],
        basis: BlobIdentity::Unknown,
        resolves_to: vec!["src/lib.rs::helper".into()],
    };
    let report = suspects(&root, &graph, "helper", "HEAD~1", "HEAD", 3);
    assert!(!report.complete);
    assert!(
        report.suspects.is_empty(),
        "an unreadable repository yields no suspects, and says why"
    );
    assert!(
        !report.unavailable.is_empty(),
        "a failure must be recorded, not swallowed"
    );
}

/// A path the graph names but the revision does not contain. One unreadable
/// file must not abort the whole analysis.
#[test]
fn a_missing_file_is_recorded_and_the_rest_still_runs() {
    let repo = init_repo("missing-file");
    write(&repo, "src/lib.rs", V1);
    let base = commit(&repo, "v1");
    write(&repo, "src/lib.rs", V2);
    let head = commit(&repo, "v2");

    let head_blob = git(&repo, &["rev-parse", &format!("{head}:src/lib.rs")]);
    let mut symbols = spans_for(V2);
    symbols.push(GraphSymbol {
        qualified_name: "src/gone.rs::ghost".into(),
        file_path: "src/gone.rs".into(),
        span_start: 0,
        span_end: 10,
        body_exact: None,
    });
    let graph = FakeGraph {
        symbols,
        cone: vec![
            ConeEntry {
                qualified_name: "src/lib.rs::helper".into(),
                file_path: "src/lib.rs".into(),
                distance: 0,
            },
            ConeEntry {
                qualified_name: "src/gone.rs::ghost".into(),
                file_path: "src/gone.rs".into(),
                distance: 1,
            },
        ],
        basis: BlobIdentity::Blob(head_blob),
        resolves_to: vec!["src/lib.rs::helper".into()],
    };

    let report = suspects(&repo, &graph, "helper", &base, &head, 3);
    assert!(
        !report.complete,
        "the missing file must be recorded: {:?}",
        report.unavailable
    );
    assert!(
        report.unavailable.iter().any(|u| matches!(
            u,
            dc_regress::Unavailable::BlameRefused { path, .. } if path == "src/gone.rs"
        )),
        "the refusal names the file that could not be read: {:?}",
        report.unavailable
    );
    assert!(
        !report.suspects.is_empty(),
        "the readable half of the cone still produced an answer"
    );
}

/// Non-UTF-8 content has no lines to attribute, and must be named rather than
/// silently producing a blame of replacement characters.
#[test]
fn a_binary_file_in_the_cone_is_refused_by_name() {
    let repo = init_repo("binary");
    write(&repo, "src/lib.rs", V1);
    std::fs::write(repo.join("blob.bin"), [0x00, 0xff, 0xfe, 0x00, 0x80]).unwrap();
    let base = commit(&repo, "v1");
    write(&repo, "src/lib.rs", V2);
    let head = commit(&repo, "v2");

    let head_blob = git(&repo, &["rev-parse", &format!("{head}:src/lib.rs")]);
    let mut symbols = spans_for(V2);
    symbols.push(GraphSymbol {
        qualified_name: "blob.bin::data".into(),
        file_path: "blob.bin".into(),
        span_start: 0,
        span_end: 4,
        body_exact: None,
    });
    let graph = FakeGraph {
        symbols,
        cone: vec![
            ConeEntry {
                qualified_name: "src/lib.rs::helper".into(),
                file_path: "src/lib.rs".into(),
                distance: 0,
            },
            ConeEntry {
                qualified_name: "blob.bin::data".into(),
                file_path: "blob.bin".into(),
                distance: 1,
            },
        ],
        basis: BlobIdentity::Blob(head_blob),
        resolves_to: vec!["src/lib.rs::helper".into()],
    };

    let report = suspects(&repo, &graph, "helper", &base, &head, 3);
    assert!(
        report.unavailable.iter().any(|u| matches!(
            u,
            dc_regress::Unavailable::BlameRefused { path, .. } if path == "blob.bin"
        )),
        "binary content is refused by name: {:?}",
        report.unavailable
    );
}

/// A non-ASCII path must survive the round trip through git's argv and the
/// `-z` splitting, and still match the graph's `file_path`.
#[test]
fn a_unicode_path_is_matched_not_mangled() {
    let repo = init_repo("unicode-path");
    let path = "src/café/módulo.rs";
    write(&repo, path, V1);
    let base = commit(&repo, "v1");
    write(&repo, path, V2);
    let head = commit(&repo, "v2");

    let head_blob = git(&repo, &["rev-parse", &format!("{head}:{path}")]);
    let helper_start = V2.find("pub fn helper").unwrap();
    let helper_end = V2[helper_start..].find("}\n").unwrap() + helper_start + 1;
    let graph = FakeGraph {
        symbols: vec![GraphSymbol {
            qualified_name: format!("{path}::helper"),
            file_path: path.into(),
            span_start: helper_start,
            span_end: helper_end,
            body_exact: None,
        }],
        cone: vec![ConeEntry {
            qualified_name: format!("{path}::helper"),
            file_path: path.into(),
            distance: 0,
        }],
        basis: BlobIdentity::Blob(head_blob),
        resolves_to: vec![format!("{path}::helper")],
    };

    let report = suspects(&repo, &graph, "helper", &base, &head, 3);
    assert!(
        report.complete,
        "a non-ASCII path is an ordinary path: {:?}",
        report.unavailable
    );
    assert_eq!(report.suspects.len(), 1);
    assert_eq!(report.suspects[0].commit, head);
}

/// A path beginning with a dash must be a path, not a flag. `--` is what makes
/// that true and this is what proves it is there.
#[test]
fn a_path_that_looks_like_a_flag_is_treated_as_a_path() {
    let repo = init_repo("dash-path");
    let path = "--weird.rs";
    write(&repo, path, V1);
    let base = commit(&repo, "v1");
    write(&repo, path, V2);
    let head = commit(&repo, "v2");

    let head_blob = git(&repo, &["rev-parse", &format!("{head}:{path}")]);
    let helper_start = V2.find("pub fn helper").unwrap();
    let helper_end = V2[helper_start..].find("}\n").unwrap() + helper_start + 1;
    let graph = FakeGraph {
        symbols: vec![GraphSymbol {
            qualified_name: format!("{path}::helper"),
            file_path: path.into(),
            span_start: helper_start,
            span_end: helper_end,
            body_exact: None,
        }],
        cone: vec![ConeEntry {
            qualified_name: format!("{path}::helper"),
            file_path: path.into(),
            distance: 0,
        }],
        basis: BlobIdentity::Blob(head_blob),
        resolves_to: vec![format!("{path}::helper")],
    };

    let report = suspects(&repo, &graph, "helper", &base, &head, 3);
    assert!(
        report.complete,
        "`--` terminates options, so a leading-dash path is a path: {:?}",
        report.unavailable
    );
    assert_eq!(report.suspects.len(), 1);
}

/// Running the same question twice over the same repository must produce
/// byte-identical reports. A report that cannot reproduce itself is not
/// evidence.
#[test]
fn the_same_question_twice_gives_the_same_answer() {
    let repo = init_repo("determinism");
    write(&repo, "src/lib.rs", V1);
    let base = commit(&repo, "v1");
    write(&repo, "src/lib.rs", V2);
    let head = commit(&repo, "v2");

    let head_blob = git(&repo, &["rev-parse", &format!("{head}:src/lib.rs")]);
    let graph = FakeGraph {
        symbols: spans_for(V2),
        cone: vec![
            ConeEntry {
                qualified_name: "src/lib.rs::helper".into(),
                file_path: "src/lib.rs".into(),
                distance: 0,
            },
            ConeEntry {
                qualified_name: "src/lib.rs::unrelated".into(),
                file_path: "src/lib.rs".into(),
                distance: 2,
            },
        ],
        basis: BlobIdentity::Blob(head_blob),
        resolves_to: vec!["src/lib.rs::helper".into()],
    };

    let first = suspects(&repo, &graph, "helper", &base, &head, 3);
    let second = suspects(&repo, &graph, "helper", &base, &head, 3);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
}
