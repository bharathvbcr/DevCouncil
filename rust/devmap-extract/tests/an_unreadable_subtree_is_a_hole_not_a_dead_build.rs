//! One directory the walker cannot open must not cost the whole map.
//!
//! Discovery already has the right answer for a file it cannot read: it is
//! recorded as `DiscoverySkipReason::Unreadable`, `is_refusal` counts it as
//! coverage loss, and the other few thousand files are indexed. A *directory*
//! it cannot open took the opposite path — `for result in walker { let entry =
//! result?; }` propagated the walker's error out of `collect_sources_with_report`
//! and the build stopped.
//!
//! Measured with the release binary on a tree holding one `chmod 000`
//! directory beside eight ordinary sources:
//!
//! ```text
//! $ devmap build <root>            # directory mode 000
//!   {"error":"…/noperm: IO error … Permission denied (os error 13)"}   exit 1
//! $ devmap --json status
//!   {"is_fresh": false, "node_count": 0,
//!    "degraded_reason": "this store holds no generation: nothing has been indexed yet"}
//!
//! $ chmod 755 <root>/noperm && devmap build <root>
//!   {"files_indexed": 5, "discovery_refused_files": 3, …}
//!     src/over_limit.py:  Oversized { bytes: 1048577, limit: 1048576 }
//!     src/unreadable.py:  Unreadable { reason: "Permission denied (os error 13)" }
//! ```
//!
//! Two spellings of "this indexer could not read that path", one recorded as
//! coverage loss and one fatal. The fatal one leaves an agent with no map at
//! all, which is the outcome every other refusal in this file exists to avoid:
//! a root-owned build directory, a `.git` object pack with odd modes, or a
//! mount that lost `+x` are ordinary, and none of them is a reason to refuse to
//! describe the rest of the repository.
//!
//! The root itself is different and stays fatal: a root that cannot be opened
//! has not been examined at all, and answering "zero sources, complete" for one
//! is precisely the check-that-could-not-run reporting as a check that passed.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use devmap_extract::collect_sources_with_report;
use devmap_extract::model::DiscoverySkipReason;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-unreadable-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Restore the mode whatever the test does, so a failure cannot leave a
/// directory the harness cannot clean up.
struct Unreadable(PathBuf);

impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn a_directory_the_walker_cannot_open_is_a_refusal_and_the_rest_is_indexed() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("dir");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("locked")).unwrap();
    for index in 0..8 {
        fs::write(
            root.join(format!("src/mod_{index}.py")),
            format!("def fn_{index}():\n    return {index}\n"),
        )
        .unwrap();
    }
    fs::write(
        root.join("locked/inside.py"),
        "def hidden():\n    return 1\n",
    )
    .unwrap();
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = Unreadable(root.join("locked"));
    if !restriction_holds(root.join("locked")) {
        return;
    }

    let (sources, report) = match collect_sources_with_report(&root) {
        Ok(answer) => answer,
        Err(error) => panic!(
            "one unreadable directory ended the whole discovery pass with {error}; \
             the eight readable sources beside it were never indexed, and an \
             unreadable *file* in the same tree is recorded as a refusal and \
             skipped"
        ),
    };

    assert_eq!(
        sources.len(),
        8,
        "every readable source must still be collected, got {:?}",
        sources.iter().map(|(path, _)| path).collect::<Vec<_>>()
    );

    // Class A: skipping it silently would be worse than failing. The path has
    // to be named, and it has to count as loss.
    let refusal = report
        .skipped_paths
        .iter()
        .find(|(path, _)| path.starts_with("locked"))
        .unwrap_or_else(|| {
            panic!(
                "the unreadable directory must be recorded, not passed over: {:?}",
                report.skipped_paths
            )
        });
    assert!(
        matches!(refusal.1, DiscoverySkipReason::Unreadable { .. }),
        "an unopenable directory is unreadable, not something else: {refusal:?}"
    );
    assert!(
        refusal.1.is_refusal(),
        "a subtree nobody could read is a hole in the graph"
    );
    assert_eq!(
        report.refusals().count(),
        1,
        "exactly the unreadable directory, once: {:?}",
        report.skipped_paths
    );
}

/// The OFF direction. A root that cannot be opened has not been examined, and
/// "zero sources" is not the answer to that.
#[test]
fn a_root_that_cannot_be_opened_is_still_an_error() {
    let missing = scratch("root").join("does-not-exist");
    let error = collect_sources_with_report(&missing)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_else(|| {
            panic!(
                "a missing root answered instead of failing, so an empty index \
                    reads as a repository with nothing in it"
            )
        });
    assert!(
        error.contains("does-not-exist"),
        "the failure must name the root it could not open: {error}"
    );

    use std::os::unix::fs::PermissionsExt;
    let locked = scratch("lockedroot");
    fs::write(locked.join("a.py"), "def a():\n    return 1\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = Unreadable(locked.clone());
    if !restriction_holds(&locked) {
        return;
    }
    assert!(
        collect_sources_with_report(&locked).is_err(),
        "a root this process cannot open must fail rather than report an empty \
         repository"
    );
}

/// A root that exists and is not a directory was never examined either.
///
/// `ignore::WalkBuilder` walks a file root by yielding the file itself, so a
/// build pointed at a regular file "ran": zero candidates, zero sources, and a
/// generation that reads as a repository with nothing in it. Measured through
/// the release binary: `devmap build <regular file>` exited 0 with
/// `files_indexed: 0` and wrote a generation, while `devmap build <missing
/// path>` exited 1 — two roots the walk could not examine, one a dead build
/// and one a passing one.
#[test]
fn a_root_that_is_not_a_directory_is_refused_not_walked_as_an_empty_tree() {
    let dir = scratch("fileroot");
    let file = dir.join("README.md");
    fs::write(&file, "# not a repository\n").unwrap();
    let error = collect_sources_with_report(&file)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_else(|| {
            panic!(
                "a regular file answered as a root instead of failing, so an empty \
                 index reads as a repository with nothing in it"
            )
        });
    assert!(
        error.contains("README.md") && error.contains("not a directory"),
        "the failure must name the root and say what is wrong with it: {error}"
    );
}

/// The same `result?`, in the second walk of the same tree.
///
/// `devmap build` runs `collect_sources_with_report` and then
/// `collect_go_modules(path)?` over the same root, so fixing only the first
/// left the build failing exactly as before — measured through the release
/// binary, which still exited 1 on the `chmod 000` directory after discovery
/// had already recorded its two file refusals and moved on.
///
/// Go module discovery is enrichment: a directory it cannot open holds no
/// `go.mod` it can read, and that is not a reason to refuse to describe the
/// repository. The loss is disclosed by `collect_sources_with_report`, which
/// walks the same tree and records the same directory as a refusal.
#[test]
fn go_module_discovery_steps_over_a_subtree_it_cannot_open() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("gomod");
    fs::create_dir_all(root.join("svc")).unwrap();
    fs::create_dir_all(root.join("locked")).unwrap();
    fs::write(
        root.join("svc/go.mod"),
        "module example.com/svc\n\ngo 1.22\n",
    )
    .unwrap();
    fs::write(root.join("locked/go.mod"), "module example.com/hidden\n").unwrap();
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = Unreadable(root.join("locked"));
    if !restriction_holds(root.join("locked")) {
        return;
    }

    let modules = devmap_extract::collect_go_modules(&root).unwrap_or_else(|error| {
        panic!(
            "one unreadable directory ended Go module discovery with {error}, \
             which fails the whole `devmap build` that calls it"
        )
    });
    assert_eq!(
        modules.len(),
        1,
        "the readable module must still be found: {modules:?}"
    );
    assert_eq!(modules[0].prefix, "example.com/svc");
}

/// And a root it cannot open is still an error here too, for the same reason.
#[test]
fn go_module_discovery_still_fails_on_a_root_it_cannot_open() {
    let missing = scratch("gomodroot").join("does-not-exist");
    assert!(
        devmap_extract::collect_go_modules(&missing).is_err(),
        "a missing root must fail rather than report a repository with no Go modules"
    );
}

/// A manifest the walk reaches and cannot read is a refusal, not a module
/// that is not there.
///
/// The directory case above steps over a subtree and leaves the disclosure to
/// discovery. A *file* is different: the walk opened the directory, found the
/// `go.mod`, and `let Ok(source) = fs::read_to_string(path) else { continue; }`
/// dropped it — so its module prefix left import resolution and nothing
/// anywhere recorded that it had been there.
#[test]
fn an_unreadable_go_mod_is_recorded_with_its_path_and_reason() {
    use devmap_extract::model::DiscoverySkipReason;
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("gomodfile");
    fs::create_dir_all(root.join("svc")).unwrap();
    fs::create_dir_all(root.join("locked")).unwrap();
    fs::write(root.join("svc/go.mod"), "module example.com/svc\n").unwrap();
    fs::write(root.join("locked/go.mod"), "module example.com/locked\n").unwrap();
    fs::set_permissions(
        root.join("locked/go.mod"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let _restore = Unreadable(root.join("locked/go.mod"));
    if !restriction_holds(root.join("locked/go.mod")) {
        return;
    }

    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert_eq!(
        manifests
            .go_modules
            .iter()
            .map(|module| module.prefix.as_str())
            .collect::<Vec<_>>(),
        vec!["example.com/svc"],
        "the readable module is still found: {manifests:?}"
    );
    assert_eq!(
        manifests.refusals.len(),
        1,
        "the unreadable one is recorded, once: {manifests:?}"
    );
    let (path, reason) = &manifests.refusals[0];
    assert_eq!(path, "locked/go.mod");
    assert!(
        matches!(reason, DiscoverySkipReason::Unreadable { reason } if reason.contains("denied")),
        "the reason is the read error, in discovery's wording: {reason:?}"
    );
    assert!(
        reason.is_refusal(),
        "and it is coverage loss, by the one rule that decides: {reason:?}"
    );
    assert!(manifests.examined.contains("locked/go.mod"));
    assert!(
        !manifests.repaired("locked/go.mod"),
        "a manifest still refused has not been repaired"
    );
}

/// The case discovery cannot cover at all: an `extends` target under
/// `node_modules/`, which discovery never walks and the config reader reads
/// anyway.
///
/// Reproduced through the binary before this was written: `tsconfig.json`
/// extending `@acme/base/tsconfig.json` with the base `chmod 000` built a
/// generation whose `status` reported every coverage gap at zero and no
/// degraded reason, while the `paths` the base declared were simply gone.
#[test]
fn an_unreadable_extends_target_outside_discovery_is_recorded() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("tsextends");
    let base_dir = root.join("node_modules/@acme/base");
    fs::create_dir_all(&base_dir).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{ "extends": "@acme/base/tsconfig.json" }"#,
    )
    .unwrap();
    let base = base_dir.join("tsconfig.json");
    fs::write(
        &base,
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@core/*": ["src/core/*"] } } }"#,
    )
    .unwrap();
    let readable = devmap_extract::collect_project_manifests(&root).unwrap();
    assert!(
        !readable.ts_projects.is_empty() && readable.refusals.is_empty(),
        "fixture assumption: readable, the base supplies the mapping — {readable:?}"
    );

    for locked in [base.clone(), root.join("node_modules/@acme")] {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let restore = Unreadable(locked.clone());
        if !restriction_holds(&locked) {
            return;
        }
        let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
        assert!(
            manifests.ts_projects.is_empty(),
            "fixture assumption: with the base unreadable no mapping survives — {manifests:?}"
        );
        assert_eq!(
            manifests
                .refusals
                .iter()
                .map(|(path, reason)| (path.as_str(), reason.is_refusal()))
                .collect::<Vec<_>>(),
            vec![("node_modules/@acme/base/tsconfig.json", true)],
            "`{}` locked: the config the chain could not read is named, where it \
             used to vanish exactly like a config that declares no `paths` — \
             {manifests:?}",
            locked.display()
        );
        assert!(
            !devmap_extract::is_indexable_source("node_modules/@acme/base/tsconfig.json"),
            "fixture assumption: discovery never walks this path, so no other pass records it"
        );
        drop(restore);
    }

    // Repaired, it leaves: and `repaired` says so for the path only this pass
    // reads, not for the root config discovery owns the verdict on.
    let repaired = devmap_extract::collect_project_manifests(&root).unwrap();
    assert!(repaired.refusals.is_empty(), "{repaired:?}");
    assert!(repaired.repaired("node_modules/@acme/base/tsconfig.json"));
    assert!(
        !repaired.repaired("tsconfig.json"),
        "discovery walks the root config and keeps the verdict on it"
    );
}

/// A config that is simply not there is still not a refusal: an `extends`
/// naming a package nobody installed is "nothing there", and charging it
/// would leave every checkout without `node_modules` permanently degraded.
#[test]
fn a_missing_extends_target_is_not_a_refusal() {
    let root = scratch("tsmissing");
    fs::write(
        root.join("tsconfig.json"),
        r#"{ "extends": "@acme/base/tsconfig.json", "references": [{ "path": "./pkg" }] }"#,
    )
    .unwrap();
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert!(manifests.refusals.is_empty(), "{manifests:?}");
}

/// Whether the mode bits just set actually refuse this process.
///
/// Root, and filesystems that ignore mode bits, read through `0o000` and write
/// through read-only, and then the condition this test builds cannot exist.
/// The test says so on stderr and stops rather than failing on its
/// environment; wherever the restriction holds, every assertion runs.
#[cfg(unix)]
fn restriction_holds(path: impl AsRef<std::path::Path>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let path = path.as_ref();
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    let mode = meta.permissions().mode() & 0o777;
    let refused = if mode & 0o444 == 0 {
        if meta.is_dir() {
            std::fs::read_dir(path).is_err()
        } else {
            std::fs::File::open(path).is_err()
        }
    } else if meta.is_dir() {
        let probe = path.join(".devmap-permission-probe");
        match std::fs::File::create(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                false
            }
            Err(_) => true,
        }
    } else {
        std::fs::OpenOptions::new().append(true).open(path).is_err()
    };
    if !refused {
        eprintln!(
            "skipped: mode {mode:o} on {} does not refuse this process (running as root?); \
             the condition this test needs cannot be built here",
            path.display()
        );
    }
    refused
}

// --- The manifest pass under hostile trees -----------------------------------
//
// The manifest pass is the build's third walk of the tree, and the only one
// that did not ask the repository's containment rule anything: it read with
// `fs::read_to_string`, which follows every link it meets. Each row below is a
// shape a real checkout arrives in — `npm link`, a pnpm workspace, a dangling
// link left by a deleted package, an editor's lock symlink loop — and each must
// end in one of two answers: read (the bytes are the repository's), or refused
// with a reason. Never read from somewhere else, never silently dropped, never
// a hang.

/// A directory beside the root, not under it, for the far end of a link.
fn outside_of(root: &std::path::Path, name: &str) -> PathBuf {
    let outside = root.with_file_name(format!(
        "{}-{name}",
        root.file_name().and_then(|n| n.to_str()).unwrap_or("root")
    ));
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).unwrap();
    outside
}

const BASE_CONFIG: &str =
    r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@core/*": ["src/core/*"] } } }"#;

/// A root `tsconfig.json` extending `@acme/base/tsconfig.json`, with the
/// package directory left for the caller to build.
fn ts_root(name: &str) -> PathBuf {
    let root = scratch(name);
    fs::create_dir_all(root.join("node_modules/@acme")).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{ "extends": "@acme/base/tsconfig.json" }"#,
    )
    .unwrap();
    root
}

fn refused_paths(manifests: &devmap_extract::ProjectManifests) -> Vec<&str> {
    assert!(
        manifests
            .refusals
            .iter()
            .all(|(_, reason)| reason.is_refusal()),
        "every manifest refusal is coverage loss by the one rule: {manifests:?}"
    );
    manifests
        .refusals
        .iter()
        .map(|(path, _)| path.as_str())
        .collect()
}

#[test]
fn a_go_mod_linked_out_of_the_repository_is_refused_not_read() {
    let root = scratch("gomod-escape");
    let outside = outside_of(&root, "gomod-escape-out");
    fs::write(outside.join("go.mod"), "module evil.example/x\n").unwrap();
    fs::create_dir_all(root.join("svc")).unwrap();
    std::os::unix::fs::symlink(outside.join("go.mod"), root.join("svc/go.mod")).unwrap();

    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert!(
        manifests.go_modules.is_empty(),
        "a module the repository does not contain must not reach import \
         resolution: {manifests:?}"
    );
    assert_eq!(refused_paths(&manifests), vec!["svc/go.mod"]);
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn an_extends_target_linked_out_of_the_repository_is_refused_not_read() {
    // Both shapes: the config file itself a link, and an ancestor directory a
    // link (`npm link`), which a check on the final component cannot see.
    for linked_directory in [false, true] {
        let root = ts_root(if linked_directory {
            "ts-escape-dir"
        } else {
            "ts-escape-file"
        });
        let outside = outside_of(&root, "out");
        fs::write(outside.join("tsconfig.json"), BASE_CONFIG).unwrap();
        if linked_directory {
            std::os::unix::fs::symlink(&outside, root.join("node_modules/@acme/base")).unwrap();
        } else {
            fs::create_dir_all(root.join("node_modules/@acme/base")).unwrap();
            std::os::unix::fs::symlink(
                outside.join("tsconfig.json"),
                root.join("node_modules/@acme/base/tsconfig.json"),
            )
            .unwrap();
        }
        let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
        assert!(
            manifests.ts_projects.is_empty(),
            "linked_directory={linked_directory}: a mapping read from outside the \
             repository must not govern its files — {manifests:?}"
        );
        assert_eq!(
            refused_paths(&manifests),
            vec!["node_modules/@acme/base/tsconfig.json"],
            "linked_directory={linked_directory}"
        );
        let _ = fs::remove_dir_all(&outside);
    }
}

/// The OFF direction, and the shape most repositories that link a package at
/// all are in: a pnpm / yarn workspace links `node_modules/@acme/base` to a
/// package inside the same repository. Those bytes are the repository's.
#[test]
fn an_extends_target_linked_inside_the_repository_is_read() {
    let root = ts_root("ts-workspace-link");
    fs::create_dir_all(root.join("packages/base")).unwrap();
    fs::write(root.join("packages/base/tsconfig.json"), BASE_CONFIG).unwrap();
    std::os::unix::fs::symlink("../../packages/base", root.join("node_modules/@acme/base"))
        .unwrap();
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert!(
        refused_paths(&manifests).is_empty(),
        "a workspace link is not a refusal: {manifests:?}"
    );
    // `packages/base` governs itself too; the claim is about the root.
    assert!(
        manifests
            .ts_projects
            .iter()
            .any(|project| project.dir.is_empty() && !project.mappings.is_empty()),
        "and its mapping governs the root: {manifests:?}"
    );
}

#[test]
fn a_dangling_or_looping_manifest_link_is_refused_not_passed_over() {
    // An `extends` target that dangles, and one in a link loop.
    for shape in ["dangling", "loop"] {
        let root = ts_root(&format!("ts-{shape}"));
        let base = root.join("node_modules/@acme/base");
        fs::create_dir_all(&base).unwrap();
        if shape == "dangling" {
            std::os::unix::fs::symlink(base.join("gone.json"), base.join("tsconfig.json")).unwrap();
        } else {
            std::os::unix::fs::symlink(base.join("other.json"), base.join("tsconfig.json"))
                .unwrap();
            std::os::unix::fs::symlink(base.join("tsconfig.json"), base.join("other.json"))
                .unwrap();
        }
        let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
        assert_eq!(
            refused_paths(&manifests),
            vec!["node_modules/@acme/base/tsconfig.json"],
            "{shape}: a link that names nothing readable is a refusal, as discovery \
             makes it for source — {manifests:?}"
        );
    }

    // A `go.mod` that dangles, in a directory discovery does not walk.
    let root = scratch("gomod-dangling");
    fs::create_dir_all(root.join("build")).unwrap();
    std::os::unix::fs::symlink(root.join("build/nowhere"), root.join("build/go.mod")).unwrap();
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert_eq!(
        refused_paths(&manifests),
        vec!["build/go.mod"],
        "{manifests:?}"
    );
}

#[test]
fn a_manifest_that_is_not_utf8_is_refused_with_the_decode_error() {
    let root = ts_root("ts-not-utf8");
    fs::create_dir_all(root.join("node_modules/@acme/base")).unwrap();
    fs::write(
        root.join("node_modules/@acme/base/tsconfig.json"),
        b"{ \"compilerOptions\": { \"baseUrl\": \"caf\xe9\" } }",
    )
    .unwrap();
    fs::create_dir_all(root.join("dist")).unwrap();
    fs::write(root.join("dist/go.mod"), b"module example.com/caf\xe9\n").unwrap();
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert_eq!(
        refused_paths(&manifests),
        vec!["dist/go.mod", "node_modules/@acme/base/tsconfig.json"],
        "sorted by path, both named: {manifests:?}"
    );
    for (_, reason) in &manifests.refusals {
        assert!(
            reason.to_string().to_ascii_lowercase().contains("utf-8"),
            "the reason says what was wrong with the bytes: {reason}"
        );
    }
}

/// Past the ceiling is an ordinary skip, exactly as discovery's `Oversized` is
/// — charging it would leave every repository with a generated mega-config
/// permanently partial.
#[test]
fn an_oversized_manifest_is_skipped_not_refused() {
    let root = ts_root("ts-oversized");
    fs::create_dir_all(root.join("node_modules/@acme/base")).unwrap();
    let mut huge = String::from("{ \"compilerOptions\": {} ");
    huge.push_str(&" ".repeat(devmap_extract::tsconfig::MAX_CONFIG_BYTES as usize + 1));
    huge.push('}');
    fs::write(root.join("node_modules/@acme/base/tsconfig.json"), huge).unwrap();
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert!(refused_paths(&manifests).is_empty(), "{manifests:?}");
}

/// A cycle through an unreadable member: the unreadable config is named once,
/// and the cycle still terminates.
#[test]
#[cfg(unix)]
fn an_extends_cycle_through_an_unreadable_member_names_it_once() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("ts-cycle");
    fs::write(root.join("tsconfig.json"), r#"{ "extends": "./a.json" }"#).unwrap();
    fs::write(root.join("a.json"), r#"{ "extends": "./b.json" }"#).unwrap();
    fs::write(root.join("b.json"), r#"{ "extends": "./a.json" }"#).unwrap();
    fs::set_permissions(root.join("b.json"), fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = Unreadable(root.join("b.json"));
    if !restriction_holds(root.join("b.json")) {
        return;
    }
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert_eq!(refused_paths(&manifests), vec!["b.json"], "{manifests:?}");
}

/// A FIFO wherever a manifest is expected must not hang the pass. Run in a
/// bounded child: a hang here is the failure, and an in-process test would
/// hang the suite instead of failing it.
#[test]
fn a_fifo_manifest_cannot_block_the_manifest_pass() {
    use devmap_extract::subprocess::{run_bounded, Bounds};
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child.args(["--ignored", "--exact", "fifo_manifest_child", "--nocapture"]);
    let result = run_bounded(
        &mut child,
        Bounds {
            deadline: std::time::Duration::from_secs(10),
            stdout_cap: 16384,
            stderr_cap: 16384,
        },
    )
    .expect("the manifest pass must finish within the deadline");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "bounded child of a_fifo_manifest_cannot_block_the_manifest_pass"]
fn fifo_manifest_child() {
    let root = ts_root("ts-fifo");
    fs::create_dir_all(root.join("node_modules/@acme/base")).unwrap();
    fs::create_dir_all(root.join("svc")).unwrap();
    for fifo in [
        root.join("node_modules/@acme/base/tsconfig.json"),
        root.join("svc/go.mod"),
        root.join("svc/tsconfig.json"),
    ] {
        let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }
    // A link to a FIFO survives both of the walk's own filters — its entry is
    // a symlink — and so is the shape that reaches `read_manifest` itself.
    fs::create_dir_all(root.join("build")).unwrap();
    std::os::unix::fs::symlink(root.join("svc/go.mod"), root.join("build/go.mod")).unwrap();
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    assert!(manifests.go_modules.is_empty() && manifests.ts_projects.is_empty());
    assert!(
        manifests.examined.contains("build/go.mod"),
        "fixture assumption: the linked FIFO reached the read decision — {manifests:?}"
    );
    // Not a source and not a hole — discovery steps over a FIFO without a
    // word, as `CandidateKind::Other`, and so does this pass.
    assert!(refused_paths(&manifests).is_empty(), "{manifests:?}");
}

/// The manifest walk runs in parallel, and its answer is compared across
/// builds: the same tree must produce the same `ProjectManifests`, field for
/// field, however the threads interleave. Repeated over a tree wide enough
/// that the walk's workers genuinely race.
#[test]
#[cfg(unix)]
fn the_parallel_manifest_walk_is_deterministic() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("deterministic");
    let mut locked = Vec::new();
    for index in 0..200 {
        let dir = root.join(format!("mods/m{index:03}/sub"));
        fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("go.mod");
        fs::write(&manifest, format!("module example.com/m{index}\n")).unwrap();
        if index % 17 == 0 {
            fs::set_permissions(&manifest, fs::Permissions::from_mode(0o000)).unwrap();
            locked.push(Unreadable(manifest));
        }
    }
    for index in 0..100 {
        let dir = root.join(format!("web/p{index:03}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("tsconfig.json"),
            format!(r#"{{ "compilerOptions": {{ "paths": {{ "@p{index}/*": ["./src/*"] }} }} }}"#),
        )
        .unwrap();
    }
    if locked.iter().any(|lock| !restriction_holds(&lock.0)) {
        return;
    }
    let first = devmap_extract::collect_project_manifests(&root).unwrap();
    assert_eq!(first.go_modules.len(), 200 - locked.len());
    assert_eq!(first.refusals.len(), locked.len());
    assert_eq!(first.ts_projects.len(), 100);
    assert_eq!(first.examined.len(), 300);
    for round in 0..25 {
        assert_eq!(
            devmap_extract::collect_project_manifests(&root).unwrap(),
            first,
            "round {round}: the same tree produced a different answer"
        );
    }
}
