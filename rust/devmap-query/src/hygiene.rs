//! Cleanup eligibility is deliberately narrower than disk-usage classification.
use std::io::Read;
use std::path::Path;

fn marker(path: &Path, prefix: &str) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut text = String::new();
    file.take(1024).read_to_string(&mut text).is_ok() && text.starts_with(prefix)
}

/// Directory names that are persistent state or dependencies wherever they sit.
fn protected_name(part: &str) -> bool {
    matches!(
        part.to_ascii_lowercase().as_str(),
        ".git"
            | ".venv"
            | "venv"
            | ".tox"
            | "node_modules"
            | "pods"
            | ".terraform"
            | ".devcouncil"
            | ".devmap"
            | ".gitnexus"
            | ".claude"
            | ".cursor"
            | ".agents"
            | ".gemini"
            | ".antigravity"
            | ".opencode"
            | ".gopath"
            | ".gomodcache"
            | ".gradle"
            | ".pnpm-store"
            | ".npm-cache"
            | ".yarn-cache"
            | ".cache"
            | "tmp"
            | ".tmp"
            | "temp"
            | "logs"
            | "log"
    )
}

/// Names a package author can legitimately choose. Below Cargo's own layout a
/// directory with one of these names is a crate (`build/log/<hash>/`), not a
/// place where someone keeps state. Dot-names cannot be crate names, so
/// `.tmp` and the VCS, environment and agent-state names are not listed.
fn crate_name_shaped(part: &str) -> bool {
    matches!(
        part.to_ascii_lowercase().as_str(),
        "tmp" | "temp" | "log" | "logs"
    )
}

/// Directories Cargo creates under `<profile>/`, whose children it names.
const CARGO_LAYOUT_DIRS: [&str; 4] = ["build", "deps", ".fingerprint", "incremental"];

/// Name protection over `path`, except that components from index `exempt_from`
/// on may be crate names.
fn protected_components(path: &str, exempt_from: usize) -> bool {
    path.split(['/', '\\']).enumerate().any(|(index, part)| {
        protected_name(part) && !(index >= exempt_from && crate_name_shaped(part))
    })
}

pub fn protected_artifact(path: &str) -> bool {
    protected_components(path, usize::MAX)
}

/// Whether `dir` is a Cargo target directory: Cargo's cache tag and the
/// compiler-info file it writes at the root. The one definition shared by
/// producer recognition and entry protection.
pub fn is_cargo_target(dir: &Path) -> bool {
    marker(
        &dir.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55",
    ) && dir.join(".rustc_info.json").is_file()
}

/// The index of the first component below Cargo's layout directory, for a path
/// shaped `<profile>/<layout>/...` or `<triple>/<profile>/<layout>/...`.
fn below_cargo_layout(relative: &Path) -> Option<usize> {
    let text = relative.to_string_lossy();
    let parts: Vec<&str> = text.split(['/', '\\']).collect();
    // A path that steps sideways or is not relative is not a layout path.
    if parts.iter().any(|p| matches!(*p, "" | "." | "..")) {
        return None;
    }
    [1, 2]
        .into_iter()
        .find(|&at| parts.get(at).is_some_and(|p| CARGO_LAYOUT_DIRS.contains(p)))
        .map(|at| at + 1)
}

/// Recognized names are a discovery hint; producer evidence, the index, age,
/// complete traversal and activity are checked separately before removal.
pub fn local_provider(repo: &Path, relative: &str) -> Result<&'static str, String> {
    if relative.is_empty()
        || relative.len() > 4096
        || relative.contains('\\')
        || relative.chars().any(char::is_control)
        || Path::new(relative).is_absolute()
        || Path::new(relative)
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err("Artifact must be a bounded literal relative path".into());
    }
    if protected_artifact(relative) {
        return Err(
            "Preserved: dependencies, environments, shared caches or persistent local state."
                .into(),
        );
    }
    let path = repo.join(relative);
    let parent = path.parent().ok_or("Missing artifact parent")?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid artifact name")?;
    let has = |marker: &str| parent.join(marker).is_file();
    match name {
        n if (n == "target" || n.starts_with("target-")) && has("Cargo.toml") => {
            if !is_cargo_target(&path) {
                return Err(
                    "Cargo target markers are missing; this directory requires manual review."
                        .into(),
                );
            }
            Ok("Rust / Cargo")
        }
        "__pycache__" => Ok("Python bytecode"),
        ".gocache"
            if marker(
                &path.join("README"),
                "This directory holds cached build artifacts from the Go build system.",
            ) =>
        {
            Ok("Go build cache")
        }
        ".pytest_cache" | ".mypy_cache" | ".ruff_cache" if path.join("CACHEDIR.TAG").is_file() => {
            Ok("Python tooling")
        }
        ".next" | ".nuxt" | ".output" | ".svelte-kit" | ".parcel-cache" | ".turbo" | ".vite"
            if has("package.json") =>
        {
            Ok("JavaScript / TypeScript")
        }
        "build" if has("build.gradle") || has("build.gradle.kts") => Ok("JVM / Gradle"),
        "target" if has("pom.xml") => Ok("JVM / Maven"),
        "cmake-build-debug" | "cmake-build-release"
            if has("CMakeLists.txt") && path.join("CMakeCache.txt").is_file() =>
        {
            Ok("C / C++ / CMake")
        }
        "obj"
            if std::fs::read_dir(parent)
                .map_err(|e| e.to_string())?
                .take(512)
                .any(|e| {
                    e.is_ok_and(|e| {
                        matches!(
                            e.path().extension().and_then(|s| s.to_str()),
                            Some("csproj" | "fsproj" | "vbproj")
                        )
                    })
                }) =>
        {
            Ok(".NET intermediates")
        }
        _ => Err("No verified cleanup adapter for this directory. Review it manually.".into()),
    }
}

pub const CACHE_ADVICE: &[(&str, &str, &str)] = &[
    ("Cargo", "Automatic global GC", "Cargo 1.88+ expires unused global data during substantial online commands. Leave it enabled; target directories need separate review. Prefer per-project targets or a compiler cache over one shared target directory."),
    ("Go", "Automatic build-cache expiry", "Go already expires unused build-cache entries. Clear the build cache only to reclaim space or investigate cgo changes. Preserve module downloads and fuzzing inputs."),
    ("Gradle", "Automatic cache retention", "Use Gradle's built-in cache retention. Keep module downloads, wrappers and daemon state; avoid invoking project clean tasks during discovery."),
    ("Swift / Xcode", "Preserve package checkouts", "Use swift package clean or Xcode's build cleanup after stopping builds. Do not delete all of .build: it also holds dependency checkouts and repository state."),
    ("Python / Node", "Keep environments and dependencies", "Prefer uv cache prune, npm cache verify and pnpm store prune. Virtual environments, node_modules, model weights and datasets remain protected."),
];

/// Shared by generated agent guides and host applications; guidance is not an
/// authorization grant. Hosts still own consent, activity checks and execution.
pub const AGENT_RULES: &str = "## Repository hygiene\n\n\
- Keep generated output in ignored, producer-owned directories. Use per-workspace Cargo targets and the native Go build cache; reuse valid caches instead of clearing them after every build.\n\
- Before cleanup, measure the exact paths and check producer markers, the Git index, ignore rules, nested repositories, symlinks, active tasks, open files and retention. An unavailable or partial check is a refusal, not approval.\n\
- Preserve source, uncommitted work, secrets, environments, dependencies, models, datasets, agent state and Git history. Never use home-wide rm sweeps or git clean -X as routine hygiene.\n\
- Prefer Cargo automatic GC, Go automatic expiry, npm cache verify, uv cache prune and pnpm store prune for shared caches. Explain download and rebuild costs; never silently change global target/cache configuration.\n\
- Agents must not enable or widen cleanup schedules. Only execute a user's saved scope and limits, with fresh checks immediately before mutation; stop when files, policy or activity change.\n\
- Record what was measured, removed, skipped and failed, including incomplete scans and logical versus physical bytes. Cancellation does not restore entries already removed.";

/// Bounds are common to every host; hosts may tighten them but cannot treat
/// invalid values as an instruction to disable a limit.
pub fn validate_retention(days: u32, scheduled: bool) -> Result<(), String> {
    let minimum = if scheduled { 7 } else { 1 };
    if !(minimum..=3650).contains(&days) {
        return Err(format!("Retention must be between {minimum} and 3650 days"));
    }
    Ok(())
}

#[cfg(test)]
mod hygiene_tests {
    use super::{protected_artifact, validate_retention, AGENT_RULES};
    #[test]
    fn every_host_gets_bounded_retention_and_preservation_rules() {
        for days in [0, 3651, u32::MAX] {
            assert!(validate_retention(days, false).is_err());
        }
        for days in 1..7 {
            assert!(validate_retention(days, true).is_err());
        }
        assert!(validate_retention(7, true).is_ok());
        assert!(validate_retention(3650, false).is_ok());
        for path in [
            ".venv",
            "a/.terraform/state",
            "node_modules",
            "nested/.CLAUDE/state",
        ] {
            assert!(protected_artifact(path), "{path}");
        }
        assert!(AGENT_RULES.contains("Agents must not enable or widen"));
        assert!(AGENT_RULES.contains("unavailable or partial"));
    }
    #[test]
    fn generated_guides_include_the_canonical_rules_without_overwriting_custom_guides() {
        let text =
            crate::guides::agent_guide_text(&serde_json::json!({}), "m.json", "g.json", "s.sqlite");
        assert!(text.contains(AGENT_RULES));
        assert_eq!(text.matches("## Repository hygiene").count(), 1);
    }
}

/// Conservative preservation of known state, credential and data formats.
/// This is evidence of a refusal, not proof that other files are disposable.
///
/// With no knowledge of what the path sits in, every directory name is
/// protected. Use [`protected_entry_in`] when the path is inside a candidate
/// whose root is known.
pub fn protected_entry(relative: &Path) -> bool {
    protected_entry_from(relative, usize::MAX)
}

/// [`protected_entry`] for `relative` inside the candidate directory
/// `candidate_root`.
///
/// Inside a recognised Cargo target ([`is_cargo_target`]), the components below
/// `<profile>/{build,deps,.fingerprint,incremental}` (or the same under
/// `<triple>/<profile>/`) are names crates.io chose, so a dependency called
/// `log` or `tmp` does not protect the candidate. Everything else keeps its
/// answer: the target's own top level and the layout directories' parents,
/// every file name and extension protection, and every directory outside a
/// Cargo target. A root that cannot be read is not a Cargo target.
pub fn protected_entry_in(candidate_root: &Path, relative: &Path) -> bool {
    if !protected_entry(relative) {
        return false;
    }
    // Only the crate-name list can be waived, and only below the layout, so the
    // filesystem is consulted only for the few entries where that decides it.
    match below_cargo_layout(relative) {
        Some(from) if !protected_entry_from(relative, from) => !is_cargo_target(candidate_root),
        _ => true,
    }
}

fn protected_entry_from(relative: &Path, exempt_from: usize) -> bool {
    if protected_components(&relative.to_string_lossy(), exempt_from) {
        return true;
    }
    let Some(name) = relative.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    let name = name.to_ascii_lowercase();
    let extension = relative
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name.starts_with(".env")
        || matches!(
            name.as_str(),
            "pyvenv.cfg" | "credentials.json" | "service-account.json" | "id_rsa" | "id_ed25519"
        )
        || matches!(
            extension.as_str(),
            "safetensors"
                | "gguf"
                | "ckpt"
                | "pem"
                | "key"
                | "pt"
                | "pth"
                | "onnx"
                | "h5"
                | "hdf5"
                | "parquet"
                | "arrow"
        )
}

/// Audit a list of tracked relative repository paths against hygiene invariants.
/// Flags any tracked path that belongs to protected artifacts (dependencies,
/// environments, shared caches, temporary files, logs, credential stores, or secret configs).
/// Returns a list of violation messages.
pub fn audit_tracked_hygiene(_repo: &Path, tracked_relative_paths: &[&str]) -> Vec<String> {
    let mut violations = Vec::new();
    for path in tracked_relative_paths {
        let p = Path::new(path);
        if protected_artifact(path) || protected_entry(p) {
            violations.push(format!(
                "Hygiene violation: tracked path '{path}' matches protected artifact or secret pattern"
            ));
        }
    }
    violations
}

#[cfg(test)]
mod boundary_tests {
    use super::{audit_tracked_hygiene, local_provider, protected_artifact, protected_entry};
    use std::path::Path;
    #[test]
    fn public_adapter_rejects_non_relative_and_parent_paths_before_probing() {
        for path in [
            "",
            "/tmp/__pycache__",
            "../__pycache__",
            "a/../../__pycache__",
            "a\\..\\__pycache__",
        ] {
            assert!(local_provider(Path::new("/repo"), path).is_err(), "{path}");
        }
    }
    #[test]
    fn preserves_state_on_case_insensitive_and_windows_style_paths() {
        for path in [
            "nested/.CLAUDE/state",
            "nested\\.claude\\state",
            ".VENV",
            "Pods",
        ] {
            assert!(protected_artifact(path), "{path}");
        }
        for path in [
            ".ENV.production",
            "MODEL.ONNX",
            "table.PARQUET",
            "weights.PTH",
            "id_ed25519",
        ] {
            assert!(protected_entry(Path::new(path)), "{path}");
        }
    }
    #[test]
    fn audit_tracked_hygiene_flags_protected_and_secret_artifacts() {
        let paths = [
            "src/main.rs",
            "Cargo.toml",
            ".env.local",
            "node_modules/package.json",
            "keys/id_ed25519",
            "tests/test.py",
        ];
        let violations = audit_tracked_hygiene(Path::new("/repo"), &paths);
        assert_eq!(violations.len(), 3);
        assert!(violations.iter().any(|v| v.contains(".env.local")));
        assert!(violations
            .iter()
            .any(|v| v.contains("node_modules/package.json")));
        assert!(violations.iter().any(|v| v.contains("keys/id_ed25519")));
    }
}

#[cfg(test)]
mod cargo_target_tests {
    use super::{protected_entry, protected_entry_in};
    use std::path::{Path, PathBuf};

    const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# cache dir\n";

    /// A scratch directory that removes itself, so a failed assertion leaves nothing behind.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "devmap-hygiene-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cargo_target(scratch: &Scratch) -> PathBuf {
        let root = scratch.0.join("target");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("CACHEDIR.TAG"), CACHEDIR_TAG).unwrap();
        std::fs::write(root.join(".rustc_info.json"), "{}").unwrap();
        root
    }

    fn plain_target(scratch: &Scratch) -> PathBuf {
        let root = scratch.0.join("target");
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    const BUILD_SCRIPT_DIRS: [&str; 6] = [
        "release/build/log/0123abcd/out",
        "release/build/tmp/0123abcd/out/lib.rs",
        "debug/build/logs/0123abcd",
        "aarch64-apple-darwin/release/build/temp/0123abcd/out",
        "x86_64-unknown-linux-gnu/debug/incremental/log/s-1-2",
        "release/.fingerprint/temp/lib-temp.json",
    ];

    #[test]
    fn current_behaviour_without_context_refuses_crate_named_build_directories() {
        for path in BUILD_SCRIPT_DIRS {
            assert!(protected_entry(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn crate_named_directories_below_cargo_layout_are_not_protected_in_a_cargo_target() {
        let scratch = Scratch::new("crate-named");
        let root = cargo_target(&scratch);
        for path in BUILD_SCRIPT_DIRS {
            assert!(!protected_entry_in(&root, Path::new(path)), "{path}");
        }
        for path in [
            "debug/deps/tmp-0123abcd.d",
            "debug/deps/libtemp-0123abcd.rlib",
            "release/build/log-0123abcd/out",
        ] {
            assert!(!protected_entry_in(&root, Path::new(path)), "{path}");
        }
    }

    #[test]
    fn the_same_paths_stay_protected_without_cargo_target_markers() {
        let scratch = Scratch::new("no-markers");
        let root = plain_target(&scratch);
        for path in BUILD_SCRIPT_DIRS {
            assert!(protected_entry_in(&root, Path::new(path)), "{path}");
        }
        // One marker is not enough: both identify a Cargo target.
        std::fs::write(root.join("CACHEDIR.TAG"), CACHEDIR_TAG).unwrap();
        assert!(protected_entry_in(
            &root,
            Path::new("release/build/log/0123abcd")
        ));
        std::fs::remove_file(root.join("CACHEDIR.TAG")).unwrap();
        std::fs::write(root.join(".rustc_info.json"), "{}").unwrap();
        assert!(protected_entry_in(
            &root,
            Path::new("release/build/log/0123abcd")
        ));
        // A tag whose signature is not Cargo's does not count.
        std::fs::write(root.join("CACHEDIR.TAG"), "Signature: other\n").unwrap();
        assert!(protected_entry_in(
            &root,
            Path::new("release/build/log/0123abcd")
        ));
    }

    #[test]
    fn a_root_that_cannot_be_read_is_not_a_cargo_target() {
        let scratch = Scratch::new("missing-root");
        let missing = scratch.0.join("does-not-exist");
        assert!(protected_entry_in(
            &missing,
            Path::new("release/build/log/0123abcd")
        ));
    }

    #[test]
    fn name_protection_keeps_applying_outside_the_layout_directories() {
        let scratch = Scratch::new("outside-layout");
        let root = cargo_target(&scratch);
        for path in [
            "logs",
            "logs/run.txt",
            "tmp",
            "tmp/build/x",
            "temp/deps/x",
            "log/build/x",
            "release/log",
            "release/tmp/x",
            "release/examples/log/x",
            "release/build/../log",
            "release/build/node_modules/x",
            "release/build/x/.git/config",
            "release/build/x/.venv/bin/python",
            "release/deps/.claude/state",
            "release/build/x/.tmp/y",
            "release/build/x/.cache/y",
            "x/y/z/build/log",
            "release",
        ] {
            let expected = !matches!(path, "release");
            assert_eq!(
                protected_entry_in(&root, Path::new(path)),
                expected,
                "{path}"
            );
        }
    }

    #[test]
    fn file_protections_apply_everywhere_inside_a_cargo_target() {
        let scratch = Scratch::new("file-protections");
        let root = cargo_target(&scratch);
        for path in [
            "release/build/x/out/.env",
            "release/build/x/out/.env.local",
            "release/build/x/out/credentials.json",
            "release/build/x/out/service-account.json",
            "release/build/x/out/id_rsa",
            "release/build/x/out/id_ed25519",
            "release/build/x/out/server.pem",
            "release/build/x/out/server.KEY",
            "release/build/x/out/model.safetensors",
            "release/build/x/out/model.gguf",
            "debug/deps/weights.pt",
            "debug/deps/weights.pth",
            "release/build/log/0123abcd/out/server.pem",
            "release/build/log/0123abcd/out/.env",
            "aarch64-apple-darwin/release/build/temp/0123abcd/out/id_rsa",
            "release/build/x/out/pyvenv.cfg",
        ] {
            assert!(protected_entry_in(&root, Path::new(path)), "{path}");
        }
    }

    #[test]
    fn empty_and_non_utf8_paths_keep_their_answer() {
        let scratch = Scratch::new("odd-paths");
        let root = cargo_target(&scratch);
        assert!(protected_entry(Path::new("")));
        assert!(protected_entry_in(&root, Path::new("")));
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let odd = Path::new(OsStr::from_bytes(b"release/build/log/\xff\xfe"));
            assert!(protected_entry(odd));
            assert!(protected_entry_in(&root, odd));
        }
    }

    #[test]
    fn a_cargo_target_is_not_widened_for_unrelated_paths() {
        let scratch = Scratch::new("unrelated");
        let root = cargo_target(&scratch);
        for path in [
            "release/build/x/out/lib.rs",
            "debug/deps/libfoo.rlib",
            "CACHEDIR.TAG",
        ] {
            assert!(!protected_entry_in(&root, Path::new(path)), "{path}");
        }
    }
}
