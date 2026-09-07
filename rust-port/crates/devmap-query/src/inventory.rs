//! The repository evidence the extraction pass does not carry.
//!
//! `repo_map.json`'s `package_managers` and `test_commands` shipped as `[]` on
//! every repository this kernel has ever mapped, marked
//! `package_managers_computed: false` / `test_commands_computed: false`. The
//! reason recorded in `manifest.rs` was accurate about the *extractions*: a
//! `.lock` file matches no language spec, so `is_indexable_source` excludes it
//! and it never reaches an `Extraction`; and a test command needs the contents
//! of `pyproject.toml` or `package.json`, not their declaration.
//!
//! It was not a reason the kernel could not answer the question. The kernel is
//! handed a repository root — `build_code_graph_value` already takes one — so
//! reading a *bounded* inventory off that root answers both from evidence this
//! producer can support, and `hotspots` needs the same treatment for a
//! different reason (repository history, which extraction also does not read).
//! Both live here so there is one owner for "look at the repository itself",
//! with one set of bounds.
//!
//! Everything here is bounded and says so when a bound bit:
//!
//!   - the marker walk descends at most [`WALK_DEPTH_CAP`] levels and visits at
//!     most [`WALK_DIR_CAP`] directories, reporting `walk_truncated` when it
//!     stops early;
//!   - a manifest larger than [`MANIFEST_READ_CAP`] is *named* in
//!     `refused_oversize` rather than silently skipped, because "could not
//!     read" and "read and found nothing" must never be the same answer;
//!   - `git log` runs once, with a deadline, a commit cap and an output cap.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Largest manifest this reader will pull into memory.
///
/// A `package.json` or `pyproject.toml` is kilobytes; a quarter of a megabyte
/// is far past any hand-written one and small enough that reading it costs
/// nothing measurable on the artifact-write path. A file past it is recorded,
/// not read.
pub const MANIFEST_READ_CAP: u64 = 256 * 1024;

/// Deepest directory level the marker walk descends to.
///
/// Marker files (`Cargo.toml`, `go.mod`, `package.json`) sit at a package root.
/// Eight levels reaches every one of them in this repository and in the
/// monorepo layouts the map is aimed at, without the walk's cost being a
/// function of how deep the deepest source tree happens to be.
pub const WALK_DEPTH_CAP: usize = 8;

/// Most directories the marker walk will open.
///
/// The bound exists so a pathological tree — a generated fixture corpus, a
/// checked-in dependency cache the skip list does not name — cannot turn an
/// artifact write into a full-disk traversal. On this repository the walk opens
/// roughly 400.
pub const WALK_DIR_CAP: usize = 20_000;

/// Hard ceiling for the churn subprocess, matching the shape
/// `devmap-store`'s `run_git_head_with_deadline` established for `rev-parse`:
/// `git` can stall on a network mount, a hook, or a lock, and unbounded it
/// would stall the artifact write behind it.
pub const CHURN_DEADLINE: Duration = Duration::from_secs(10);

/// Most commits the churn window will look at.
///
/// Paired with the date window rather than replacing it: `--since` alone is
/// unbounded on a repository with a very busy quarter, and this is what stops
/// the output being a function of commit rate.
pub const CHURN_COMMIT_CAP: usize = 5_000;

/// Most bytes of `git log` output the churn reader will accept.
pub const CHURN_OUTPUT_CAP: usize = 8 * 1024 * 1024;

/// The churn window. The Python original's `--since=90.days`, kept.
pub const CHURN_SINCE: &str = "90.days";

/// Directories the marker walk never descends into, at any depth.
///
/// `target` and `node_modules` are build and dependency output whose size is
/// unrelated to the repository's own, and both are conventionally named — a
/// source directory called `node_modules` does not exist, and one called
/// `target` is rare enough that missing a marker inside it costs less than
/// walking a multi-gigabyte cargo tree on every artifact write. `vendor` and
/// `__pycache__` are the same argument.
///
/// `dist`, `build` and `out` are deliberately *not* here and are skipped only
/// at the top level (see [`skip_dir`]), for the reason `freshness.rs` records
/// against the same names: this repository contains a source directory literally
/// named `build`, and excluding it by name at any depth drops real files.
const SKIP_DIR_ANY_DEPTH: &[&str] = &["target", "node_modules", "vendor", "__pycache__"];

/// Directories skipped only where a build tool would put them: the repository
/// root.
const SKIP_DIR_TOP_LEVEL: &[&str] = &["dist", "build", "out"];

/// Marker basenames the walk records wherever it finds them.
///
/// A nested one is real evidence: this repository's `Cargo.toml` is at
/// `rust-port/Cargo.toml` and its `go.mod` at
/// `backend/go_orchestrator/go.mod`, so a top-level-only rule would report a
/// polyglot repository as Python-only.
const NESTED_MARKERS: &[&str] = &[
    "go.mod",
    "go.sum",
    "Cargo.toml",
    "Package.swift",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "gradlew",
];

/// Marker basenames that only count at the repository root.
///
/// A lock file names the manager *this repository* is built with. One inside a
/// fixture, an example, or a vendored sub-project names that sub-project's, and
/// reporting it as the repository's is how `package_managers` comes to list six
/// managers for a repository that uses one.
const TOP_LEVEL_MARKERS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "requirements.txt",
    "uv.lock",
    "poetry.lock",
    "pyproject.toml",
    "setup.py",
    "Gemfile.lock",
    "composer.lock",
    "Podfile.lock",
    "Makefile",
    "justfile",
    "ruff.toml",
    ".ruff.toml",
    "mypy.ini",
];

/// What the repository declares about how it is built and checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoInventory {
    /// Package managers named by a manifest or lock file, in rule order.
    pub package_managers: Vec<String>,
    /// Test, lint and typecheck commands the manifests name, in rule order.
    pub test_commands: Vec<String>,
    /// Whether this scan ran at all. `false` means the two lists above are
    /// constants and nothing looked — the distinction the `*_computed` markers
    /// exist to publish.
    pub computed: bool,
    /// Why the scan did not run, when it did not.
    pub unavailable_reason: String,
    /// Manifests past [`MANIFEST_READ_CAP`], named rather than dropped.
    pub refused_oversize: Vec<String>,
    /// Whether a walk bound stopped the search before the tree was exhausted.
    pub walk_truncated: bool,
    /// Directories opened, so a reader can size the walk that produced this.
    pub directories_visited: usize,
}

impl RepoInventory {
    /// A scan that did not happen, with the reason attached.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            unavailable_reason: reason.into(),
            ..Self::default()
        }
    }
}

/// Every marker the walk found, and the one content question it answers on the
/// way (does this repository have Python tests).
#[derive(Debug, Default)]
struct Markers {
    top_level: BTreeSet<String>,
    nested: BTreeSet<String>,
    has_python_test: bool,
    truncated: bool,
    directories_visited: usize,
}

impl Markers {
    fn top(&self, name: &str) -> bool {
        self.top_level.contains(name)
    }

    /// A marker at the root or anywhere below it.
    fn anywhere(&self, name: &str) -> bool {
        self.nested.contains(name) || self.top_level.contains(name)
    }

    fn any_gradle(&self) -> bool {
        [
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
            "gradlew",
        ]
        .iter()
        .any(|name| self.anywhere(name))
    }
}

/// Whether the walk descends into `name` at `depth`.
fn skip_dir(name: &str, depth: usize) -> bool {
    // Every dot-directory. `.git` alone is larger than most repositories, and
    // `.venv`, `.tox`, `.mypy_cache`, `.devcouncil` and `.devmap` are all
    // output rather than source. A marker file inside a dot-directory is
    // configuration for a tool, not a declaration by the repository.
    if name.starts_with('.') {
        return true;
    }
    if SKIP_DIR_ANY_DEPTH.contains(&name) {
        return true;
    }
    depth == 0 && SKIP_DIR_TOP_LEVEL.contains(&name)
}

/// Find every marker file, bounded in depth and in directories opened.
fn walk_markers(root: &Path) -> Markers {
    let mut found = Markers::default();
    // (directory, depth). An explicit stack rather than recursion: the depth cap
    // bounds it either way, but a stack cannot be turned into a stack overflow
    // by a symlink loop the cap happens not to catch.
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(root.to_path_buf(), 0)];

    while let Some((directory, depth)) = stack.pop() {
        if found.directories_visited >= WALK_DIR_CAP {
            found.truncated = true;
            break;
        }
        found.directories_visited += 1;
        let Ok(entries) = std::fs::read_dir(&directory) else {
            // An unreadable directory is not a repository claim either way. It
            // is not `truncated` — nothing was cut short by *this* code's
            // bounds — and the walk carries on with what it can read.
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `file_type` does not follow symlinks, so a link to a directory is
            // not descended into. That is the conservative reading: a linked
            // tree is reachable from wherever it really lives, and following it
            // is how a walk with a depth cap still runs forever.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth + 1 > WALK_DEPTH_CAP {
                    found.truncated = true;
                    continue;
                }
                if !skip_dir(&name, depth) {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            if depth == 0 && TOP_LEVEL_MARKERS.contains(&name.as_str()) {
                found.top_level.insert(name.clone());
            }
            if NESTED_MARKERS.contains(&name.as_str()) {
                found.nested.insert(name.clone());
            }
            if !found.has_python_test && name.ends_with(".py") {
                let absolute = entry.path();
                let relative = absolute
                    .strip_prefix(root)
                    .unwrap_or(absolute.as_path())
                    .to_string_lossy()
                    .replace('\\', "/");
                // `wiring::is_test_path` is the kernel's one owner of "is this a
                // test", already used by the god-node ranking. A second rule
                // here is how the two surfaces come to disagree about the same
                // file.
                found.has_python_test = devmap_extract::wiring::is_test_path(&relative);
            }
        }
    }
    found
}

/// Read a manifest, or record that it was too large to read.
///
/// `None` covers both "absent" and "refused"; the caller distinguishes them by
/// whether the path landed in `refused`, which is the whole point of carrying
/// the list.
fn read_bounded(root: &Path, relative: &str, refused: &mut Vec<String>) -> Option<String> {
    let path = root.join(relative);
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > MANIFEST_READ_CAP {
        refused.push(relative.to_string());
        return None;
    }
    std::fs::read_to_string(&path).ok()
}

/// Whether a TOML document opens the given table.
///
/// A line scan rather than a parse: this crate has no TOML dependency, and the
/// question is only ever "is this table declared", which a section header
/// answers. A header inside a multi-line string would be a false positive; the
/// consequence is one extra suggested command, which is why this gates only the
/// optional third-party tools and never a claim about what the repository is.
fn declares_table(document: &str, table: &str) -> bool {
    document.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with(table) && trimmed[table.len()..].starts_with([']', '.'])
    })
}

/// Whether a Makefile or justfile declares a `test` target.
fn declares_test_target(document: &str) -> bool {
    document.lines().any(|line| {
        // A target sits in column 0; a recipe line is indented. Anything after
        // the colon is prerequisites, which do not change whether the target
        // exists.
        if line.starts_with([' ', '\t']) {
            return false;
        }
        let Some((head, _)) = line.split_once(':') else {
            return false;
        };
        head.split_whitespace().any(|word| word == "test")
    })
}

/// The package managers the markers name, in the ported rule order.
fn package_managers(markers: &Markers) -> Vec<String> {
    let mut managers: Vec<String> = Vec::new();
    let push = |name: &str, managers: &mut Vec<String>| {
        if !managers.iter().any(|existing| existing == name) {
            managers.push(name.to_string());
        }
    };
    // `package-lock.json` first, then a bare `package.json` — the Python
    // writer's `elif`, kept: both name npm, and the lock file is the stronger
    // evidence.
    if markers.top("package-lock.json") || markers.top("package.json") {
        push("npm", &mut managers);
    }
    if markers.top("yarn.lock") {
        push("yarn", &mut managers);
    }
    if markers.top("pnpm-lock.yaml") {
        push("pnpm", &mut managers);
    }
    if markers.top("requirements.txt") {
        push("pip", &mut managers);
    }
    // A bare `pyproject.toml` is deliberately absent from this rule.
    // `tests/unit/test_cli_commands.py:91` pins it: every Python project has
    // one, and it names no manager. Only the lock file does.
    if markers.top("uv.lock") {
        push("uv", &mut managers);
    }
    if markers.top("poetry.lock") {
        push("poetry", &mut managers);
    }
    if markers.anywhere("go.mod") || markers.anywhere("go.sum") {
        push("go mod", &mut managers);
    }
    if markers.anywhere("Cargo.toml") {
        push("cargo", &mut managers);
    }
    if markers.anywhere("Package.swift") {
        push("swiftpm", &mut managers);
    }
    if markers.any_gradle() {
        push("gradle", &mut managers);
    }
    if markers.top("Gemfile.lock") {
        push("bundler", &mut managers);
    }
    if markers.top("composer.lock") {
        push("composer", &mut managers);
    }
    if markers.top("Podfile.lock") {
        push("cocoapods", &mut managers);
    }
    managers
}

/// The commands the manifests name, in the ported rule order.
fn test_commands(root: &Path, markers: &Markers, refused: &mut Vec<String>) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    let push = |command: String, commands: &mut Vec<String>| {
        if !commands.contains(&command) {
            commands.push(command);
        }
    };

    // Node: the scripts the repository actually declares, run through the
    // manager its lock file names.
    if markers.top("package.json") {
        if let Some(text) = read_bounded(root, "package.json", refused) {
            if let Ok(document) = serde_json::from_str::<serde_json::Value>(&text) {
                let manager = if markers.top("pnpm-lock.yaml") {
                    "pnpm"
                } else if markers.top("yarn.lock") {
                    "yarn"
                } else {
                    "npm"
                };
                for key in ["test", "lint", "typecheck", "check", "type-check"] {
                    if !document["scripts"][key].is_string() {
                        continue;
                    }
                    // `npm test` is a built-in; every other npm script needs
                    // `run`. yarn and pnpm take the bare name either way.
                    if manager == "npm" && key != "test" {
                        push(format!("npm run {key}"), &mut commands);
                    } else {
                        push(format!("{manager} {key}"), &mut commands);
                    }
                }
            }
        }
    }

    // Python. The ported rule appended `ruff check .` and `mypy .` to every
    // Python project unconditionally; that is a guess about the repository's
    // toolchain rather than a reading of it, and this artifact's whole contract
    // is that a value is evidence. Both are now gated on the repository
    // declaring the tool. `pytest` keeps the original's test-path evidence and
    // gains `[tool.pytest]`, which is a declaration in its own right.
    if markers.top("pyproject.toml") || markers.top("setup.py") {
        let pyproject = if markers.top("pyproject.toml") {
            read_bounded(root, "pyproject.toml", refused).unwrap_or_default()
        } else {
            String::new()
        };
        if markers.has_python_test || declares_table(&pyproject, "[tool.pytest") {
            push("pytest".to_string(), &mut commands);
        }
        if declares_table(&pyproject, "[tool.ruff")
            || markers.top("ruff.toml")
            || markers.top(".ruff.toml")
        {
            push("ruff check .".to_string(), &mut commands);
        }
        if declares_table(&pyproject, "[tool.mypy") || markers.top("mypy.ini") {
            push("mypy .".to_string(), &mut commands);
        }
    }

    // Go and Rust keep the original's unconditional pair: `go vet` and `cargo
    // clippy` ship with their toolchains, so naming them is not a guess about
    // what the repository installed the way `ruff` and `mypy` are.
    if markers.anywhere("go.mod") {
        push("go test ./...".to_string(), &mut commands);
        push("go vet ./...".to_string(), &mut commands);
    }
    if markers.anywhere("Cargo.toml") {
        push("cargo test".to_string(), &mut commands);
        push("cargo clippy".to_string(), &mut commands);
    }
    if markers.anywhere("Package.swift") {
        push("swift test".to_string(), &mut commands);
    }
    if markers.anywhere("gradlew") {
        push("./gradlew test".to_string(), &mut commands);
    } else if markers.any_gradle() {
        push("gradle test".to_string(), &mut commands);
    }

    // Task runners, which the Python writer did not read at all: a repository
    // whose real entry point is `make test` was reported as having none.
    if markers.top("Makefile") {
        if let Some(text) = read_bounded(root, "Makefile", refused) {
            if declares_test_target(&text) {
                push("make test".to_string(), &mut commands);
            }
        }
    }
    if markers.top("justfile") {
        if let Some(text) = read_bounded(root, "justfile", refused) {
            if declares_test_target(&text) {
                push("just test".to_string(), &mut commands);
            }
        }
    }
    commands
}

/// Read the repository's own account of how it is built and checked.
pub fn scan(root: &Path) -> RepoInventory {
    if !root.is_dir() {
        return RepoInventory::unavailable(format!(
            "repository root {} is not a readable directory",
            root.display()
        ));
    }
    let markers = walk_markers(root);
    let mut refused: Vec<String> = Vec::new();
    let package_managers = package_managers(&markers);
    let test_commands = test_commands(root, &markers, &mut refused);
    refused.sort();
    refused.dedup();
    RepoInventory {
        package_managers,
        test_commands,
        computed: true,
        unavailable_reason: String::new(),
        refused_oversize: refused,
        walk_truncated: markers.truncated,
        directories_visited: markers.directories_visited,
    }
}

/// How often each file changed inside the churn window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Churn {
    /// Repo-relative path → commits touching it, within the window.
    pub commits_by_path: BTreeMap<String, u32>,
    /// Whether the history was read at all.
    pub computed: bool,
    /// Why it was not, when it was not.
    pub unavailable_reason: String,
    /// Whether a bound cut the history short, so a reader knows the counts are
    /// a lower bound rather than the window's total.
    pub truncated: bool,
}

impl Churn {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            unavailable_reason: reason.into(),
            ..Self::default()
        }
    }
}

/// One bounded `git log`, and the per-file commit counts it yields.
///
/// The kernel already shells out to `git` for `HEAD` (`freshness::git_head`,
/// and `devmap-store`'s deadline-bearing variant); this is the same subprocess
/// discipline applied to the one other question only history can answer.
///
/// Everything about the invocation is bounded: a date window, a commit cap, an
/// output cap and a wall-clock deadline. A repository with no commits, no
/// `git`, or a `git` that stalls produces `computed: false` with the reason
/// attached — never an empty map presented as a computed answer.
pub fn churn(root: &Path) -> Churn {
    let mut child = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "log",
            &format!("--since={CHURN_SINCE}"),
            &format!("--max-count={CHURN_COMMIT_CAP}"),
            "--name-only",
            "--no-renames",
            "--pretty=format:",
            // Paths NUL-terminated and unquoted, so a non-ASCII path arrives as
            // the bytes it is rather than as a C-escaped rendering that would
            // never match an extraction's `file_path`.
            "-z",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Churn::unavailable(format!("could not run git log: {error}")),
    };

    // The pipe is drained on a helper thread for the reason `devmap-store`'s
    // `run_git_head_with_deadline` records: waiting for exit before reading
    // deadlocks the moment the pipe buffer fills, and this command's output is
    // megabytes rather than a hash.
    let Some(mut pipe) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Churn::unavailable("git log produced no readable stdout".to_string());
    };
    let (sender, receiver) = std::sync::mpsc::channel::<(Vec<u8>, bool)>();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        let mut capped = false;
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let room = CHURN_OUTPUT_CAP.saturating_sub(buffer.len());
                    if read > room {
                        buffer.extend_from_slice(&chunk[..room]);
                        capped = true;
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
                Err(_) => break,
            }
        }
        let _ = sender.send((buffer, capped));
    });

    let deadline = Instant::now() + CHURN_DEADLINE;
    let payload = match receiver.recv_timeout(CHURN_DEADLINE) {
        Ok(payload) => payload,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Churn::unavailable(format!(
                "git log exceeded {CHURN_DEADLINE:?} and was killed"
            ));
        }
    };
    // The reader saw EOF, so the child is finishing. Reap it, but never wait
    // past the same deadline for it to do so.
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::yield_now(),
            Err(_) => break,
        }
    }

    let (bytes, capped) = payload;
    let text = String::from_utf8_lossy(&bytes);
    let mut commits_by_path: BTreeMap<String, u32> = BTreeMap::new();
    for entry in text.split('\0') {
        let path = entry.trim().replace('\\', "/");
        if path.is_empty() {
            continue;
        }
        *commits_by_path.entry(path).or_insert(0) += 1;
    }
    if commits_by_path.is_empty() {
        // A repository with no commits in the window, no commits at all, or no
        // git. Which one it is cannot be told apart from here without a second
        // subprocess, so the reason says exactly that rather than guessing.
        return Churn::unavailable(
            "git log named no files: no commits in the churn window, or not a \
             git repository"
                .to_string(),
        );
    }
    Churn {
        commits_by_path,
        computed: true,
        unavailable_reason: String::new(),
        truncated: capped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_pyproject_names_no_manager() {
        let mut markers = Markers::default();
        markers.top_level.insert("pyproject.toml".to_string());
        assert!(package_managers(&markers).is_empty());
    }

    #[test]
    fn a_lock_file_names_its_manager() {
        let mut markers = Markers::default();
        markers.top_level.insert("pyproject.toml".to_string());
        markers.top_level.insert("uv.lock".to_string());
        assert_eq!(package_managers(&markers), vec!["uv".to_string()]);
    }

    #[test]
    fn a_nested_cargo_toml_still_names_cargo() {
        let mut markers = Markers::default();
        markers.nested.insert("Cargo.toml".to_string());
        assert_eq!(package_managers(&markers), vec!["cargo".to_string()]);
    }

    #[test]
    fn a_lock_file_below_the_root_is_not_the_repositorys() {
        // Only `TOP_LEVEL_MARKERS` at depth 0 reach `top_level`, so a fixture's
        // `uv.lock` cannot make the whole repository a uv project.
        let mut markers = Markers::default();
        markers.nested.insert("uv.lock".to_string());
        assert!(package_managers(&markers).is_empty());
    }

    #[test]
    fn table_headers_match_the_table_and_its_subtables() {
        assert!(declares_table(
            "[tool.pytest.ini_options]\n",
            "[tool.pytest"
        ));
        assert!(declares_table("[tool.ruff]\n", "[tool.ruff"));
        assert!(declares_table("  [tool.mypy]  \n", "[tool.mypy"));
        // Not a prefix match on an unrelated table.
        assert!(!declares_table("[tool.pytest_asyncio]\n", "[tool.pytest"));
        assert!(!declares_table("[project]\n", "[tool.ruff"));
    }

    #[test]
    fn a_makefile_target_is_read_from_column_zero_only() {
        assert!(declares_test_target("test:\n\tpytest\n"));
        assert!(declares_test_target("test: lint\n\tpytest\n"));
        assert!(declares_test_target("lint test:\n\tpytest\n"));
        // A recipe line mentioning `test:` is not a target.
        assert!(!declares_test_target("all:\n\techo test: nope\n"));
        assert!(!declares_test_target("lint:\n\truff check .\n"));
    }

    #[test]
    fn build_output_directories_are_skipped_where_build_tools_put_them() {
        assert!(skip_dir("target", 3), "cargo output at any depth");
        assert!(skip_dir("node_modules", 5));
        assert!(skip_dir(".git", 0));
        assert!(skip_dir("build", 0), "top-level build output");
        // `freshness.rs`'s rule: a source directory named `build` below the
        // root is real source and must not be dropped.
        assert!(!skip_dir("build", 1));
        assert!(!skip_dir("src", 0));
    }

    #[test]
    fn an_absent_root_is_unavailable_rather_than_empty() {
        let inventory = scan(Path::new("/definitely/not/a/repository/root"));
        assert!(!inventory.computed);
        assert!(!inventory.unavailable_reason.is_empty());
        assert!(inventory.package_managers.is_empty());
    }
}
