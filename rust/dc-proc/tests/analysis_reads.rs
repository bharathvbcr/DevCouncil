//! What an analyzed repository can still make a kernel git read run: a lazy
//! fetch from a promisor remote, and a submodule's own filter driver. Neither
//! may run, and a read that needs an object the clone does not have fails
//! rather than fetching it.
#![cfg(unix)]

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dc_proc::{Bounds, Captured, Failure, GitVersion, git_version, git_with_program, run_bounded};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("dc-proc-reads-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(std::fs::canonicalize(&dir).unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the real git for fixture setup, isolated from the machine's config.
fn setup(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=a@example.test", "-c", "user.name=a"])
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A script that records each run in `marker` and exits `status`. A filter
/// (status 0) passes stdin through; anything else leaves stdin unread, since
/// an ssh transport holds it open and a reader would wait forever.
fn marker_script(dir: &Path, marker: &Path, status: u8) -> PathBuf {
    let path = dir.join("planted.sh");
    let pass = if status == 0 { "cat\n" } else { "" };
    script(
        &path,
        &format!(
            "#!/bin/sh\necho \"$0 $*\" >> '{}'\n{pass}exit {status}\n",
            marker.display()
        ),
    );
    path
}

fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    setup(dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    setup(dir, &["add", "a.txt"]);
    setup(dir, &["commit", "-qm", "base"]);
}

fn read(program: &OsStr, root: &Path, args: &[&str]) -> Result<Captured, Failure> {
    let mut command = git_with_program(program, root, args)?;
    run_bounded(
        &mut command,
        Bounds {
            deadline: Duration::from_secs(20),
            stdout_cap: 1 << 20,
            stderr_cap: 1 << 16,
        },
    )
}

fn assert_not_run(marker: &Path, what: &str) {
    if let Ok(ran) = std::fs::read_to_string(marker) {
        panic!("{what} ran a program the repository names:\n{ran}");
    }
}

/// A submodule whose own `.git/config` defines a filter driver, with a
/// same-size edit so git must hash the file through it to compare.
#[test]
fn a_submodule_filter_does_not_run_when_the_superproject_is_read() {
    let base = Scratch::new("submodule");
    let src = base.0.join("src");
    let root = base.0.join("repo");
    repo(&src);
    repo(&root);
    setup(
        &root,
        &["submodule", "add", "-q", src.to_str().unwrap(), "sub"],
    );
    setup(&root, &["commit", "-qm", "sub"]);
    let sub = root.join("sub");
    let marker = base.0.join("ran");
    let planted = marker_script(&base.0.join("bin"), &marker, 0);
    setup(
        &sub,
        &["config", "filter.planted.clean", planted.to_str().unwrap()],
    );
    setup(&sub, &["config", "filter.planted.required", "true"]);
    let sub_git_dir = PathBuf::from(setup(&sub, &["rev-parse", "--absolute-git-dir"]));
    std::fs::create_dir_all(sub_git_dir.join("info")).unwrap();
    std::fs::write(sub_git_dir.join("info/attributes"), "* filter=planted\n").unwrap();
    std::fs::write(sub.join("a.txt"), "two\n").unwrap();

    // The fixture is only evidence if an unguarded read runs the driver.
    setup(&root, &["diff", "HEAD", "--name-only"]);
    assert!(
        marker.exists(),
        "the fixture is wrong: an unguarded diff did not run the submodule's filter"
    );
    std::fs::remove_file(&marker).unwrap();

    for args in [
        &["diff", "HEAD"][..],
        &["diff", "HEAD", "--name-only", "-z"],
        &["ls-files", "-z"],
        &["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        let captured = read(OsStr::new("git"), &root, args)
            .unwrap_or_else(|failure| panic!("git {args:?}: {failure}"));
        assert_not_run(&marker, &format!("git {args:?}"));
        assert!(
            captured.status.success(),
            "git {args:?}: {}",
            captured.stderr_trimmed()
        );
    }
}

/// A blobless clone whose remote only `core.sshCommand` can reach, with
/// `core.sshCommand` and `credential.helper` planted, and an index naming
/// files the working tree lacks — so `diff` needs blobs the clone never
/// fetched.
fn partial_clone(base: &Path) -> (PathBuf, PathBuf) {
    let src = base.join("src");
    repo(&src);
    std::fs::write(src.join("a.txt"), "one\ntwo\n").unwrap();
    setup(&src, &["commit", "-qam", "second"]);
    setup(&src, &["config", "uploadpack.allowFilter", "true"]);
    let root = base.join("clone");
    setup(
        base,
        &[
            "clone",
            "-q",
            "--no-checkout",
            "--filter=blob:none",
            &format!("file://{}", src.display()),
            root.to_str().unwrap(),
        ],
    );
    setup(&root, &["read-tree", "HEAD"]);
    let marker = base.join("ran");
    let planted = marker_script(&base.join("bin"), &marker, 1);
    let planted = planted.to_str().unwrap();
    setup(
        &root,
        &[
            "config",
            "remote.origin.url",
            "ssh://example.invalid/repo.git",
        ],
    );
    setup(&root, &["config", "core.sshCommand", planted]);
    setup(&root, &["config", "credential.helper", planted]);
    (root, marker)
}

#[test]
fn a_partial_clone_read_fails_rather_than_fetching() {
    let base = Scratch::new("partial");
    let (root, marker) = partial_clone(&base.0);

    // The fixture is only evidence if an unguarded read fetches.
    let _ = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["diff", "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output();
    assert!(
        marker.exists(),
        "the fixture is wrong: an unguarded diff did not try to fetch"
    );
    std::fs::remove_file(&marker).unwrap();

    let git = dc_proc::resolve_outside(
        OsStr::new("git"),
        std::env::var_os("PATH").as_deref(),
        &root,
    )
    .unwrap();
    let version = git_version(&git)
        .unwrap()
        .expect("the installed git's version");
    assert!(
        version.honors_no_lazy_fetch(),
        "git {version} ignores GIT_NO_LAZY_FETCH; the refusal test covers it"
    );
    for args in [
        &["diff", "HEAD"][..],
        &["diff", "HEAD~1", "HEAD"],
        &["log", "-p", "-1"],
        &["blame", "HEAD", "--", "a.txt"],
    ] {
        let result = read(OsStr::new("git"), &root, args);
        assert_not_run(&marker, &format!("git {args:?}"));
        match result {
            Ok(captured) => assert!(
                !captured.status.success(),
                "git {args:?} answered without the objects it needed: {}",
                captured.stdout_lossy()
            ),
            Err(failure) => panic!("git {args:?} did not run: {failure}"),
        }
    }
}

/// A git standing in for the real one that reports `version` and otherwise
/// runs the real git.
fn fake_git(dir: &Path, version: &str) -> PathBuf {
    let real =
        dc_proc::resolve_outside(OsStr::new("git"), std::env::var_os("PATH").as_deref(), dir)
            .unwrap();
    let path = dir.join("bin").join("git");
    script(
        &path,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = version ]; then echo 'git version {version}'; exit 0; fi\n\
             exec '{}' \"$@\"\n",
            real.display()
        ),
    );
    path
}

/// D9: a git that does not honour GIT_NO_LAZY_FETCH refuses a partial
/// clone's read with a typed refusal naming its version, and still reads an
/// ordinary clone.
#[test]
fn a_git_that_ignores_no_lazy_fetch_refuses_only_a_partial_clone() {
    for (version, refuse, named) in [
        ("2.43.0", true, "2.43.0"),
        ("2.44.0", true, "2.44.0"),
        ("2.43.4", false, ""),
        ("2.44.1", false, ""),
        ("2.45.0 (Apple Git-160)", false, ""),
        ("unknown", true, "unrecognised version"),
    ] {
        let base = Scratch::new("fake-version");
        let root = base.0.join("repo");
        repo(&root);
        let git = fake_git(&base.0, version);
        for args in [&["diff", "HEAD"][..], &["rev-parse", "HEAD"]] {
            let captured = read(git.as_os_str(), &root, args).unwrap_or_else(|f| {
                panic!("git {version} refused an ordinary clone's {args:?}: {f}")
            });
            assert!(captured.status.success(), "{}", captured.stderr_trimmed());
        }
        for (key, value) in [
            ("remote.origin.promisor", "true"),
            ("extensions.partialClone", "origin"),
            ("remote.origin.partialCloneFilter", "blob:none"),
        ] {
            setup(&root, &["config", key, value]);
            for args in [
                &["diff", "HEAD"][..],
                &["rev-parse", "HEAD"],
                &["log", "-1"],
            ] {
                match git_with_program(git.as_os_str(), &root, args) {
                    Err(Failure::Refused { reason, .. }) => {
                        assert!(
                            refuse,
                            "git {version}, {key}, {args:?} was refused: {reason}"
                        );
                        assert!(
                            reason.contains(named),
                            "the refusal does not name {named}: {reason}"
                        );
                    }
                    Err(other) => panic!("git {version}, {key}, {args:?}: {other}"),
                    Ok(_) => assert!(!refuse, "git {version}, {key}, {args:?} was not refused"),
                }
            }
            setup(&root, &["config", "--unset", key]);
        }
    }
}

#[test]
fn git_versions_are_read_and_judged_as_git_releases_them() {
    for (line, want) in [
        ("git version 2.54.0 (Apple Git-157)\n", Some("2.54.0")),
        ("git version 2.39.5.windows.1", Some("2.39.5")),
        ("git version 2.45.GIT", Some("2.45.0")),
        ("git version 2.44", Some("2.44.0")),
        ("hub version 2.14.2", None),
        ("git version x.y", None),
    ] {
        assert_eq!(
            GitVersion::parse(line).map(|v| v.to_string()).as_deref(),
            want,
            "{line:?}"
        );
    }
    for (version, honours) in [
        ("1.9.0", false),
        ("2.38.5", false),
        ("2.39.3", false),
        ("2.39.4", true),
        ("2.40.1", false),
        ("2.40.2", true),
        ("2.41.1", true),
        ("2.42.1", false),
        ("2.42.2", true),
        ("2.43.3", false),
        ("2.43.4", true),
        ("2.44.0", false),
        ("2.44.1", true),
        ("2.45.0", true),
        ("2.54.0", true),
        ("3.0.0", true),
    ] {
        let v = GitVersion::parse(&format!("git version {version}")).unwrap();
        assert_eq!(v.honors_no_lazy_fetch(), honours, "{version}");
    }
}

/// The version is asked once per git binary per process.
#[test]
fn the_git_version_is_asked_once_per_binary() {
    let base = Scratch::new("version-once");
    let root = base.0.join("repo");
    repo(&root);
    let real = dc_proc::resolve_outside(
        OsStr::new("git"),
        std::env::var_os("PATH").as_deref(),
        &root,
    )
    .unwrap();
    let log = base.0.join("calls");
    let git = base.0.join("bin").join("git");
    script(
        &git,
        &format!(
            "#!/bin/sh\n[ \"$1\" = version ] && echo version >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.display()
        ),
    );
    for _ in 0..3 {
        let captured = read(git.as_os_str(), &root, &["rev-parse", "HEAD"]).unwrap();
        assert!(captured.status.success());
    }
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(calls.matches("version").count(), 1, "{calls}");
}

/// The version probe runs before the read a caller bounded, so it carries a
/// bound shorter than any of theirs: a git that stalls on `version` is killed
/// at [`dc_proc::GIT_VERSION_DEADLINE`], not waited for.
#[test]
fn a_git_that_stalls_on_version_is_killed_at_the_probe_deadline() {
    let base = Scratch::new("stalled-version");
    let git = base.0.join("bin").join("git");
    script(&git, "#!/bin/sh\nexec sleep 30\n");
    let started = std::time::Instant::now();
    let failure = git_version(&git).expect_err("a stalled git has no version");
    let elapsed = started.elapsed();
    assert!(matches!(failure, Failure::Deadline { .. }), "{failure}");
    assert!(
        elapsed < dc_proc::GIT_VERSION_DEADLINE + Duration::from_secs(1),
        "the probe waited {elapsed:?}"
    );
    assert!(dc_proc::GIT_VERSION_DEADLINE < dc_proc::GIT_HEAD_DEADLINE);
}
