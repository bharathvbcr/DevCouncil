//! Hostile trees through the same scan → extract → resolve → analyse path the
//! accuracy fixtures use (`common::build`).
//!
//! The fixture gate is a tidy tree with one obvious target. These tests ask
//! the questions that tree does not: a second symbol with the same name, a
//! letter that only looks the same, a sibling the walker must refuse, a path
//! the filesystem will store in a different normalisation, and two builds of
//! one tree at the same time. A high-confidence edge to the wrong file is a
//! false edge. A direct same-file call with no high edge is a miss. A panic
//! or a walk that outlives the budget is a failure of the path itself.
//!
//! Cancellation is not on `scan_tree` or `Resolver::resolve_all`. The budget
//! is the wedge check this path can actually fail.

mod common;

use common::{build, Built};
use devmap_extract::model::{DiscoverySkipReason, EdgeKind};
use devmap_extract::{scan_tree, MAX_SOURCE_BYTES};
use devmap_resolve::UnresolvedClass;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BUDGET: Duration = Duration::from_secs(20);
const HIGH_MILLIS: i64 = 900;

struct TempTree {
    root: PathBuf,
    outside: Vec<PathBuf>,
}

impl TempTree {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "devmap-accuracy-stress-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap_or_else(|e| panic!("{}: {e}", root.display()));
        Self {
            root,
            outside: Vec::new(),
        }
    }

    fn write(&self, rel: &str, body: impl AsRef<[u8]>) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{}: {e}", parent.display()));
        }
        fs::write(&path, body).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    fn outside_file(&mut self, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "devmap-accuracy-stress-outside-{}-{}",
            std::process::id(),
            self.outside.len()
        ));
        fs::write(&path, body).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        self.outside.push(path.clone());
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        for path in &self.outside {
            let _ = fs::remove_file(path);
        }
    }
}

fn timed(dir: &Path) -> Built {
    let started = Instant::now();
    let built = build(dir);
    let elapsed = started.elapsed();
    assert!(
        elapsed <= BUDGET,
        "{} took {elapsed:?}, over the {BUDGET:?} budget",
        dir.display()
    );
    assert_closed(&built);
    built
}

fn assert_closed(built: &Built) {
    for edge in &built.resolution.edges {
        let millis = edge.confidence.to_millis();
        assert!(
            (0..=1000).contains(&millis),
            "confidence out of range: {millis} on {}",
            edge.source_symbol
        );
        for path in [&edge.source_file, &edge.target_file] {
            assert!(
                !path.contains('\0') && !path.contains("..") && !path.starts_with('/'),
                "edge path leaves the tree or carries a NUL: {path}"
            );
        }
    }
}

fn symbol_named(symbol: &str, name: &str) -> bool {
    let tail = symbol.rsplit("::").next().unwrap_or(symbol);
    tail.rsplit('.').next().unwrap_or(tail) == name
}

fn dump(built: &Built) -> String {
    let mut lines = Vec::new();
    for edge in &built.resolution.edges {
        if matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References) {
            lines.push(format!(
                "edge {} -{:?}-> {} ({} ms)",
                edge.source_symbol,
                edge.edge_kind,
                edge.target_symbol,
                edge.confidence.to_millis()
            ));
        }
    }
    for row in &built.resolution.unresolved {
        lines.push(format!(
            "ledger {} calls {} class {:?}",
            row.source_symbol, row.callee_name, row.class
        ));
    }
    if lines.is_empty() {
        "(no calls, references, or ledger rows)".to_string()
    } else {
        lines.join("\n")
    }
}

struct Namesake {
    label: &'static str,
    files: Vec<(&'static str, &'static str)>,
    caller_file: &'static str,
    caller: &'static str,
    callee: &'static str,
    foreign_file: &'static str,
    require_local: bool,
}

/// `require_local` is for a call the resolver already binds. A method call it
/// still cannot type must stay an honest ledger row (`UninferredReceiver`,
/// `Unresolved`, or `UnresolvedMember`) and must not grow a high edge to the
/// other file. A later bind of the local method still passes.
fn assert_local_namesake(case: &Namesake, require_local: bool) {
    let tree = TempTree::new(case.label);
    for (path, body) in &case.files {
        tree.write(path, body);
    }
    let built = timed(&tree.root);
    let local: Vec<_> = built
        .resolution
        .edges
        .iter()
        .filter(|edge| {
            matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References)
                && edge.confidence.to_millis() >= HIGH_MILLIS
                && edge.source_file.ends_with(case.caller_file)
                && symbol_named(&edge.source_symbol, case.caller)
                && edge.target_file.ends_with(case.caller_file)
                && symbol_named(&edge.target_symbol, case.callee)
        })
        .collect();
    let foreign: Vec<_> = built
        .resolution
        .edges
        .iter()
        .filter(|edge| {
            matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References)
                && edge.confidence.to_millis() >= HIGH_MILLIS
                && edge.source_file.ends_with(case.caller_file)
                && symbol_named(&edge.source_symbol, case.caller)
                && edge.target_file.ends_with(case.foreign_file)
                && symbol_named(&edge.target_symbol, case.callee)
        })
        .collect();
    assert!(
        foreign.is_empty(),
        "{label}: high edge from {caller} reached the foreign namesake in {foreign_file}\n{}",
        dump(&built),
        label = case.label,
        caller = case.caller,
        foreign_file = case.foreign_file,
    );
    if require_local || !local.is_empty() {
        assert!(
            !local.is_empty(),
            "{label}: {caller} has no high edge to local {callee} in {file}\n{}",
            dump(&built),
            label = case.label,
            caller = case.caller,
            callee = case.callee,
            file = case.caller_file,
        );
        return;
    }
    let honest = built.resolution.unresolved.iter().any(|row| {
        row.source_file.ends_with(case.caller_file)
            && symbol_named(&row.source_symbol, case.caller)
            && (row.callee_name == case.callee || symbol_named(&row.callee_name, case.callee))
            && matches!(
                row.class,
                UnresolvedClass::UninferredReceiver
                    | UnresolvedClass::Unresolved
                    | UnresolvedClass::UnresolvedMember
            )
    });
    assert!(
        honest,
        "{label}: {caller} -> {callee} is a silent miss (no local high edge, no unattributed ledger row)\n{}",
        dump(&built),
        label = case.label,
        caller = case.caller,
        callee = case.callee,
    );
}

fn go_mod() -> &'static str {
    "module example.com/stress\n\ngo 1.22\n"
}

#[test]
fn a_high_edge_stays_on_the_local_namesake() {
    let cases = [
        Namesake {
            label: "go-bare",
            files: vec![
                ("go.mod", go_mod()),
                (
                    "a/a.go",
                    "package a\nfunc Local() int { return 1 }\nfunc Caller() int { return Local() }\n",
                ),
                ("b/b.go", "package b\nfunc Local() int { return 2 }\n"),
            ],
            caller_file: "a/a.go",
            caller: "Caller",
            callee: "Local",
            foreign_file: "b/b.go",
            require_local: true,
        },
        Namesake {
            label: "go-method",
            files: vec![
                ("go.mod", go_mod()),
                (
                    "a/a.go",
                    "package a\ntype Box struct{}\nfunc (Box) Ping() int { return 1 }\nfunc Caller(box Box) int { return box.Ping() }\n",
                ),
                (
                    "b/b.go",
                    "package b\ntype Box struct{}\nfunc (Box) Ping() int { return 2 }\n",
                ),
            ],
            caller_file: "a/a.go",
            caller: "Caller",
            callee: "Ping",
            foreign_file: "b/b.go",
            require_local: true,
        },
        Namesake {
            label: "rust-bare",
            files: vec![
                ("src/lib.rs", "mod a;\nmod b;\n"),
                (
                    "src/a.rs",
                    "pub fn local() -> i32 { 1 }\npub fn caller() -> i32 { local() }\n",
                ),
                ("src/b.rs", "pub fn local() -> i32 { 2 }\n"),
            ],
            caller_file: "src/a.rs",
            caller: "caller",
            callee: "local",
            foreign_file: "src/b.rs",
            require_local: true,
        },
        Namesake {
            label: "rust-method",
            files: vec![
                ("src/lib.rs", "mod a;\nmod b;\n"),
                (
                    "src/a.rs",
                    "pub struct Box;\nimpl Box { pub fn ping(&self) -> i32 { 1 } }\npub fn caller(box: &Box) -> i32 { box.ping() }\n",
                ),
                (
                    "src/b.rs",
                    "pub struct Box;\nimpl Box { pub fn ping(&self) -> i32 { 2 } }\n",
                ),
            ],
            caller_file: "src/a.rs",
            caller: "caller",
            callee: "ping",
            foreign_file: "src/b.rs",
            require_local: false,
        },
        Namesake {
            label: "python-bare",
            files: vec![
                (
                    "left.py",
                    "def local() -> int:\n    return 1\n\ndef caller() -> int:\n    return local()\n",
                ),
                ("right.py", "def local() -> int:\n    return 2\n"),
            ],
            caller_file: "left.py",
            caller: "caller",
            callee: "local",
            foreign_file: "right.py",
            require_local: true,
        },
        Namesake {
            label: "python-method",
            files: vec![
                (
                    "left.py",
                    "class Box:\n    def ping(self) -> int:\n        return 1\n\ndef caller(box: Box) -> int:\n    return box.ping()\n",
                ),
                (
                    "right.py",
                    "class Box:\n    def ping(self) -> int:\n        return 2\n",
                ),
            ],
            caller_file: "left.py",
            caller: "caller",
            callee: "ping",
            foreign_file: "right.py",
            require_local: true,
        },
        Namesake {
            label: "ts-method",
            files: vec![
                (
                    "left.ts",
                    "export class Box { ping(): number { return 1 } }\nexport function caller(box: Box): number { return box.ping() }\n",
                ),
                (
                    "right.ts",
                    "export class Box { ping(): number { return 2 } }\n",
                ),
            ],
            caller_file: "left.ts",
            caller: "caller",
            callee: "ping",
            foreign_file: "right.ts",
            require_local: false,
        },
        Namesake {
            label: "swift-method",
            files: vec![
                (
                    "Left.swift",
                    "class Box { func ping() -> Int { return 1 } }\nfunc caller(box: Box) -> Int { return box.ping() }\n",
                ),
                (
                    "Right.swift",
                    "class Box { func ping() -> Int { return 2 } }\n",
                ),
            ],
            caller_file: "Left.swift",
            caller: "caller",
            callee: "ping",
            foreign_file: "Right.swift",
            require_local: false,
        },
        Namesake {
            label: "kotlin-method",
            files: vec![
                (
                    "Left.kt",
                    "class Box { fun ping(): Int = 1 }\nfun caller(box: Box): Int = box.ping()\n",
                ),
                ("Right.kt", "class Box { fun ping(): Int = 2 }\n"),
            ],
            caller_file: "Left.kt",
            caller: "caller",
            callee: "ping",
            foreign_file: "Right.kt",
            require_local: false,
        },
    ];

    let mut failures = Vec::new();
    for case in &cases {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_local_namesake(case, case.require_local)
        }));
        if let Err(payload) = result {
            let text = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "panic payload was not a string".to_string());
            failures.push(format!("{}: {text}", case.label));
        }
    }
    assert!(
        failures.is_empty(),
        "{} namesake case(s) failed:\n{}",
        failures.len(),
        failures.join("\n---\n")
    );
}

#[test]
fn a_confusable_letter_is_a_different_symbol() {
    let cyrillic_a = '\u{0430}';
    let foreign = format!("func L{cyrillic_a}tin() int {{ return 2 }}\n");
    let tree = TempTree::new("confusable");
    tree.write("go.mod", go_mod());
    tree.write(
        "a/a.go",
        "package a\nfunc Latin() int { return 1 }\nfunc Caller() int { return Latin() }\n",
    );
    tree.write("b/b.go", format!("package b\n{foreign}"));
    let built = timed(&tree.root);
    let wrong: Vec<_> = built
        .resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.confidence.to_millis() >= HIGH_MILLIS
                && edge.source_file.ends_with("a/a.go")
                && symbol_named(&edge.source_symbol, "Caller")
                && edge.target_symbol.chars().any(|c| c == cyrillic_a)
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "Caller resolved to the cyrillic lookalike\n{}",
        dump(&built)
    );
    assert!(
        built.resolution.edges.iter().any(|edge| {
            matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References)
                && edge.confidence.to_millis() >= HIGH_MILLIS
                && edge.source_file.ends_with("a/a.go")
                && symbol_named(&edge.source_symbol, "Caller")
                && edge.target_file.ends_with("a/a.go")
                && symbol_named(&edge.target_symbol, "Latin")
        }),
        "Caller has no high edge to Latin\n{}",
        dump(&built)
    );
}

#[test]
fn partial_discovery_failure_still_resolves_the_sibling() {
    let tree = TempTree::new("partial");
    tree.write("go.mod", go_mod());
    tree.write(
        "keep/keep.go",
        "package keep\nfunc Local() int { return 1 }\nfunc Caller() int { return Local() }\n",
    );
    tree.write(
        "broken.go",
        b"package bad\nfunc Z() int { return 1 }\n\xff\xfe",
    );
    tree.write("garbage.go", "package bad\nfunc !!! {\n\tthis is not go\n");
    let mut huge = vec![b'#'; MAX_SOURCE_BYTES as usize + 1];
    huge.extend_from_slice(b"\n");
    tree.write("huge.go", huge);
    tree.write(
        "nul.go",
        b"package bad\nfunc Nul() int { return 1 }\n\x00\n",
    );

    let scanned = scan_tree(&tree.root).unwrap_or_else(|e| panic!("scan: {e}"));
    assert!(
        scanned
            .sources
            .iter()
            .any(|(path, _)| path.ends_with("keep/keep.go")),
        "the readable sibling was not admitted: {:?}",
        scanned.sources.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert!(
        !scanned.sources.iter().any(|(path, _)| path == "broken.go"),
        "invalid UTF-8 was admitted"
    );
    assert!(
        scanned.report.skipped_paths.iter().any(|(path, reason)| {
            path.ends_with("broken.go") && matches!(reason, DiscoverySkipReason::Unreadable { .. })
        }),
        "invalid UTF-8 must be an Unreadable skip, not a silent drop: {:?}",
        scanned.report.skipped_paths
    );
    let oversized = scanned
        .report
        .over_limit()
        .find(|(path, _, _)| path.ends_with("huge.go"));
    assert!(
        oversized.is_some(),
        "huge.go was not recorded over the source ceiling: {:?}",
        scanned.report.skipped_paths
    );

    let built = timed(&tree.root);
    assert!(
        built.resolution.edges.iter().any(|edge| {
            matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References)
                && edge.confidence.to_millis() >= HIGH_MILLIS
                && edge.source_file.ends_with("keep/keep.go")
                && symbol_named(&edge.source_symbol, "Caller")
                && edge.target_file.ends_with("keep/keep.go")
                && symbol_named(&edge.target_symbol, "Local")
        }),
        "the sibling call did not survive the refused files\n{}",
        dump(&built)
    );
}

#[test]
#[cfg(unix)]
fn a_symlink_outside_the_tree_is_not_a_symbol() {
    let mut tree = TempTree::new("symlink");
    tree.write("go.mod", go_mod());
    tree.write(
        "keep/keep.go",
        "package keep\nfunc Local() int { return 1 }\nfunc Caller() int { return Local() }\n",
    );
    let outside = tree.outside_file("package stolen\nfunc Stolen() int { return 9 }\n");
    std::os::unix::fs::symlink(&outside, tree.root.join("stolen.go"))
        .unwrap_or_else(|e| panic!("symlink: {e}"));

    let scanned = scan_tree(&tree.root).unwrap_or_else(|e| panic!("scan: {e}"));
    assert!(
        scanned
            .sources
            .iter()
            .all(|(path, source)| { !path.ends_with("stolen.go") && !source.contains("Stolen") }),
        "a symlink out of the tree was indexed: {:?}",
        scanned.sources.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert!(
        scanned.report.skipped_paths.iter().any(|(path, reason)| {
            path.ends_with("stolen.go") && matches!(reason, DiscoverySkipReason::EscapesRoot { .. })
        }),
        "the escaping symlink must be EscapesRoot, not a silent drop: {:?}",
        scanned.report.skipped_paths
    );

    let built = timed(&tree.root);
    assert!(
        built.resolution.edges.iter().all(|edge| {
            !edge.source_symbol.contains("Stolen") && !edge.target_symbol.contains("Stolen")
        }),
        "Stolen from outside the tree is in the graph\n{}",
        dump(&built)
    );
}

#[test]
fn unicode_and_odd_paths_resolve() {
    let tree = TempTree::new("paths");
    tree.write("go.mod", go_mod());
    let body = "package a\nfunc Local() int { return 1 }\nfunc Caller() int { return Local() }\n";
    let crlf = body.replace('\n', "\r\n");
    tree.write("naïve/local.go", &crlf);
    tree.write("odd/my file.go", body);
    // A dotfile under a normal directory is source. A dot path at the walk
    // root is the tooling-dir rule (`is_ignored_path`), so it is recorded as
    // NonSource and stays out of the graph.
    tree.write("pkg/.hidden.go", body);
    tree.write(".hidden.go", body);

    let scanned = scan_tree(&tree.root).unwrap_or_else(|e| panic!("scan: {e}"));
    let admitted: Vec<&str> = scanned
        .sources
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    let has = |suffix: &str| {
        admitted.iter().any(|path| {
            path.ends_with(suffix)
                || (suffix.contains('ï') && path.ends_with("local.go") && !path.is_ascii())
        })
    };
    for suffix in ["naïve/local.go", "odd/my file.go", "pkg/.hidden.go"] {
        assert!(has(suffix), "{suffix} was not admitted: {admitted:?}");
    }
    assert!(
        !admitted.contains(&".hidden.go"),
        "a root dot path must not be indexed: {admitted:?}"
    );
    assert!(
        scanned.report.skipped_paths.iter().any(|(path, reason)| {
            path == ".hidden.go" && matches!(reason, DiscoverySkipReason::NonSource)
        }),
        "a root dot path must be recorded NonSource, not dropped: {:?}",
        scanned.report.skipped_paths
    );

    let built = timed(&tree.root);
    for (file, who) in [
        ("my file.go", "space"),
        ("pkg/.hidden.go", "nested dotfile"),
    ] {
        assert!(
            built.resolution.edges.iter().any(|edge| {
                matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References)
                    && edge.confidence.to_millis() >= HIGH_MILLIS
                    && edge.source_file.ends_with(file)
                    && symbol_named(&edge.source_symbol, "Caller")
                    && edge.target_file.ends_with(file)
                    && symbol_named(&edge.target_symbol, "Local")
            }),
            "{who} path did not resolve\n{}",
            dump(&built)
        );
    }
    let unicode = built.resolution.edges.iter().any(|edge| {
        matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References)
            && edge.confidence.to_millis() >= HIGH_MILLIS
            && symbol_named(&edge.source_symbol, "Caller")
            && edge.source_file.contains("local.go")
            && !edge.source_file.is_ascii()
            && symbol_named(&edge.target_symbol, "Local")
    });
    assert!(unicode, "unicode path did not resolve\n{}", dump(&built));
    assert!(
        built
            .resolution
            .edges
            .iter()
            .all(|edge| { edge.source_file != ".hidden.go" && edge.target_file != ".hidden.go" }),
        "the root dot path is in the graph\n{}",
        dump(&built)
    );
}

#[test]
fn a_gitignored_sibling_is_absent_and_the_kept_call_resolves() {
    let tree = TempTree::new("gitignore");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&tree.root)
        .status()
        .unwrap_or_else(|e| panic!("git init: {e}"));
    assert!(status.success(), "git init: {status}");
    tree.write(".gitignore", "noise.go\n");
    tree.write("go.mod", go_mod());
    tree.write(
        "keep.go",
        "package keep\nfunc Local() int { return 1 }\nfunc Caller() int { return Local() }\n",
    );
    tree.write(
        "noise.go",
        "package noise\nfunc Local() int { return 2 }\nfunc Caller() int { return Local() }\n",
    );

    let scanned = scan_tree(&tree.root).unwrap_or_else(|e| panic!("scan: {e}"));
    assert!(
        scanned
            .sources
            .iter()
            .all(|(path, _)| !path.ends_with("noise.go")),
        "gitignored noise.go was indexed: {:?}",
        scanned.sources.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    let built = timed(&tree.root);
    assert!(
        built.resolution.edges.iter().any(|edge| {
            edge.source_file.ends_with("keep.go")
                && symbol_named(&edge.source_symbol, "Caller")
                && edge.target_file.ends_with("keep.go")
                && symbol_named(&edge.target_symbol, "Local")
                && edge.confidence.to_millis() >= HIGH_MILLIS
        }),
        "kept call did not resolve\n{}",
        dump(&built)
    );
    assert!(
        built
            .resolution
            .edges
            .iter()
            .all(|edge| !edge.source_file.ends_with("noise.go")
                && !edge.target_file.ends_with("noise.go")),
        "gitignored file is in the graph\n{}",
        dump(&built)
    );
}

#[test]
fn nested_and_wide_calls_stay_on_the_local_file() {
    let mut deep = String::from("package a\nfunc Local() int { return 1 }\nfunc Caller() int {\n");
    for _ in 0..64 {
        deep.push_str("if true {\n");
    }
    deep.push_str("return Local()\n");
    for _ in 0..64 {
        deep.push_str("}\n");
    }
    deep.push_str("return 0\n}\n");

    let mut wide = String::from("package a\nfunc Local() int { return 1 }\n");
    for i in 0..200 {
        if i + 1 == 200 {
            wide.push_str(&format!("func F{i}() int {{ return Local() }}\n"));
        } else {
            wide.push_str(&format!("func F{i}() int {{ return F{}() }}\n", i + 1));
        }
    }

    let tree = TempTree::new("shape");
    tree.write("go.mod", go_mod());
    tree.write("deep/deep.go", deep);
    tree.write("wide/wide.go", wide);
    tree.write(
        "other/other.go",
        "package other\nfunc Local() int { return 2 }\nfunc F1() int { return 3 }\n",
    );

    let built = timed(&tree.root);
    for (file, caller, callee) in [
        ("deep/deep.go", "Caller", "Local"),
        ("wide/wide.go", "F0", "F1"),
        ("wide/wide.go", "F199", "Local"),
    ] {
        assert!(
            built.resolution.edges.iter().any(|edge| {
                matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References)
                    && edge.confidence.to_millis() >= HIGH_MILLIS
                    && edge.source_file.ends_with(file)
                    && symbol_named(&edge.source_symbol, caller)
                    && edge.target_file.ends_with(file)
                    && symbol_named(&edge.target_symbol, callee)
            }),
            "{caller} -> {callee} missed in {file}\n{}",
            dump(&built)
        );
        assert!(
            built.resolution.edges.iter().all(|edge| {
                !(edge.confidence.to_millis() >= HIGH_MILLIS
                    && edge.source_file.ends_with(file)
                    && symbol_named(&edge.source_symbol, caller)
                    && edge.target_file.ends_with("other/other.go"))
            }),
            "{caller} in {file} reached other/other.go\n{}",
            dump(&built)
        );
    }
}

fn digest(built: &Built) -> Vec<String> {
    let mut rows: Vec<String> = built
        .resolution
        .edges
        .iter()
        .map(|edge| {
            format!(
                "e\t{}\t{}\t{:?}\t{}",
                edge.source_symbol,
                edge.target_symbol,
                edge.edge_kind,
                edge.confidence.to_millis()
            )
        })
        .chain(built.resolution.unresolved.iter().map(|row| {
            format!(
                "u\t{}\t{}\t{:?}",
                row.source_symbol, row.callee_name, row.class
            )
        }))
        .collect();
    rows.sort();
    rows
}

#[test]
fn parallel_builds_of_the_same_tree_agree() {
    let write = |tree: &TempTree| {
        tree.write("go.mod", go_mod());
        tree.write(
            "a/a.go",
            "package a\nfunc Local() int { return 1 }\nfunc Caller() int { return Local() }\n",
        );
        tree.write("b/b.go", "package b\nfunc Local() int { return 2 }\n");
    };
    let alone = TempTree::new("serial");
    write(&alone);
    let serial = digest(&timed(&alone.root));
    assert!(!serial.is_empty(), "serial build produced an empty graph");

    let left = TempTree::new("par-left");
    let right = TempTree::new("par-right");
    write(&left);
    write(&right);
    let (a, b) = thread::scope(|scope| {
        let a = scope.spawn(|| digest(&timed(&left.root)));
        let b = scope.spawn(|| digest(&timed(&right.root)));
        (
            a.join().expect("left build panicked"),
            b.join().expect("right build panicked"),
        )
    });
    assert_eq!(a, serial, "parallel build diverged from the serial build");
    assert_eq!(
        b, serial,
        "the two parallel builds diverged from each other"
    );
}

#[test]
fn a_missing_root_is_an_error_and_an_empty_root_is_an_empty_graph() {
    let missing = std::env::temp_dir().join(format!(
        "devmap-accuracy-stress-missing-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&missing);
    let error = scan_tree(&missing).expect_err("a missing root must not scan as an empty tree");
    assert!(
        !error.to_string().is_empty(),
        "the missing-root error must name the failure"
    );

    let tree = TempTree::new("empty");
    let built = timed(&tree.root);
    assert!(
        built.resolution.edges.is_empty() && built.resolution.unresolved.is_empty(),
        "an empty directory produced graph rows\n{}",
        dump(&built)
    );
}
