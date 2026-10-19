//! A relative repository root names the same tree, and writes to the same
//! places, as its absolute spelling.
//!
//! Measured at e01e6f0d, from a directory containing `x/`:
//!
//! ```text
//! devmap build --manifest --full x    wrote x/.devmap/codeintel/*            (right)
//!                                     and   x/x/.devmap/repo_map.json        (doubled)
//!                                           x/x/.devmap/graph/code_graph.json
//! ```
//!
//! The state directory derived from `x` was already cwd-relative, and the
//! manifest writer joined it onto the store's absolute repo root a second time.
//! The root is now resolved once, where the argument is parsed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-relroot-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn repo(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), "pub fn a() {}\n").unwrap();
    dir
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("devmap runs")
}

fn succeeded(out: &Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every file under `root`, as root-relative paths.
fn files(root: &Path) -> BTreeSet<String> {
    fn walk(base: &Path, dir: &Path, into: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, into);
            } else {
                into.insert(path.strip_prefix(base).unwrap().display().to_string());
            }
        }
    }
    let mut into = BTreeSet::new();
    walk(root, root, &mut into);
    into
}

/// The files a build leaves, minus the sqlite sidecars whose presence depends
/// on timing.
fn built(root: &Path) -> BTreeSet<String> {
    files(root)
        .into_iter()
        .filter(|name| !name.ends_with(".writer.lock"))
        .collect()
}

#[test]
fn a_relative_root_writes_the_files_an_absolute_root_writes_and_nothing_else() {
    let base = scratch("equal");
    let abs = repo(&base, "abs");
    let rel = repo(&base, "rel");
    succeeded(
        &run(
            &base,
            &["build", "--manifest", "--full", abs.to_str().unwrap()],
        ),
        "absolute build",
    );
    // Run from a directory that is neither the repository nor its parent's
    // repository, so a join against the working directory cannot coincide.
    succeeded(
        &run(&base, &["build", "--manifest", "--full", "rel"]),
        "relative build",
    );
    let expected = built(&abs);
    assert!(
        expected.iter().any(|name| name.ends_with("repo_map.json")),
        "the absolute build must have written the map: {expected:?}"
    );
    assert_eq!(
        built(&rel),
        expected,
        "a relative root must leave exactly the absolute root's files, with no doubled directory"
    );
    assert!(
        !rel.join("rel").exists(),
        "no `rel/rel` may be created under the repository"
    );
}

#[test]
fn every_spelling_of_a_relative_root_lands_in_the_same_place() {
    let base = scratch("forms");
    for (index, spelling) in ["./x", "x/", "x/../x", "./y/../x/."]
        .into_iter()
        .enumerate()
    {
        let cwd = base.join(format!("case{index}"));
        std::fs::create_dir_all(cwd.join("y")).unwrap();
        let root = repo(&cwd, "x");
        succeeded(
            &run(&cwd, &["build", "--manifest", "--full", spelling]),
            spelling,
        );
        let names = built(&root);
        assert!(
            names.contains(".devmap/repo_map.json")
                && names.contains(".devmap/graph/code_graph.json"),
            "{spelling}: artifacts belong directly under the root: {names:?}"
        );
        assert!(
            names
                .iter()
                .all(|name| name == "a.rs" || name.starts_with(".devmap/")),
            "{spelling}: nothing may be written outside .devmap/: {names:?}"
        );
    }
}

#[test]
fn a_unicode_directory_name_is_not_doubled() {
    let base = scratch("unicode");
    let root = repo(&base, "répô-ポ");
    succeeded(
        &run(&base, &["build", "--manifest", "--full", "répô-ポ"]),
        "unicode build",
    );
    let names = built(&root);
    assert!(
        names
            .iter()
            .all(|name| name == "a.rs" || name.starts_with(".devmap/")),
        "{names:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_relative_root_that_is_a_symlink_writes_into_the_tree_it_points_at() {
    let base = scratch("link");
    let real = repo(&base, "real");
    std::os::unix::fs::symlink(&real, base.join("link")).unwrap();
    succeeded(
        &run(&base, &["build", "--manifest", "--full", "link"]),
        "symlink build",
    );
    let names = built(&real);
    assert!(
        names
            .iter()
            .all(|name| name == "a.rs" || name.starts_with(".devmap/")),
        "{names:?}"
    );
    assert!(names.contains(".devmap/repo_map.json"), "{names:?}");
}

#[test]
fn a_relative_root_that_does_not_exist_is_refused_and_nothing_is_created() {
    let base = scratch("missing");
    let out = run(&base, &["build", "--manifest", "--full", "nowhere"]);
    assert!(!out.status.success(), "a missing root must be refused");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nowhere"),
        "the refusal must name the root: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        files(&base).is_empty() && !base.join("nowhere").exists(),
        "nothing may be created: {:?}",
        files(&base)
    );
}

#[test]
fn queries_given_a_relative_root_create_no_doubled_directory() {
    let base = scratch("queries");
    let root = repo(&base, "x");
    succeeded(
        &run(&base, &["build", "--manifest", "--full", "x"]),
        "build",
    );
    let before = built(&root);
    for args in [
        &["manifest", "x"][..],
        &["freshness", "x"],
        &["paths", "x"],
        &["--root", "x", "status"],
        &["--root", "./x", "manifest"],
    ] {
        succeeded(&run(&base, args), &args.join(" "));
        assert!(
            !root.join("x").exists(),
            "{args:?} created a doubled directory"
        );
    }
    let after = built(&root);
    assert!(
        after
            .iter()
            .all(|name| name == "a.rs" || name.starts_with(".devmap/")),
        "queries wrote outside .devmap/: {after:?} (was {before:?})"
    );
}
