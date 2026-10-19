//! Adversarial probes against `lane/build-relative-root` @ a74c0583.
//!
//! `root_parser()` anchors a relative root with `std::path::absolute`, which is
//! lexical: it keeps `..` and resolves no symlink. A failing test here is a
//! finding; a passing one is a claim that held.
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
        "devmap-relatk-{tag}-{}-{}",
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

fn text(out: &Output) -> String {
    format!(
        "status={:?}\nSTDOUT:\n{}\nSTDERR:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn ok(out: &Output, what: &str) {
    assert!(out.status.success(), "{what} failed: {}", text(out));
}

fn json(out: &Output, what: &str) -> serde_json::Value {
    ok(out, what);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{what}: not json ({e}): {}", text(out)))
}

fn field(value: &serde_json::Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

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
        .into_iter()
        .filter(|n| !n.ends_with(".writer.lock"))
        .collect()
}

// ---- Probe 1: `..` normalization -------------------------------------------

/// `paths` canonicalizes `root` but reports `db_path` lexically, so a `..`
/// spelling names the same store by a different string. Callers that key by the
/// reported store path (hooks, GitPulse) see two stores for one repository.
#[test]
fn p1_a_dotdot_spelling_reports_the_same_store_path_as_the_plain_one() {
    let base = scratch("p1paths");
    repo(&base, "x");
    ok(&run(&base, &["build", "--manifest", "--full", "x"]), "build");
    let plain = json(&run(&base, &["paths", "--json", "x"]), "paths x");
    let dotdot = json(&run(&base, &["paths", "--json", "x/../x"]), "paths x/../x");
    assert_eq!(field(&plain, "root"), field(&dotdot, "root"));
    assert_eq!(
        field(&plain, "db_path"),
        field(&dotdot, "db_path"),
        "one repository, two reported store identities"
    );
}

/// Same, through `status --root`, the surface GitPulse hooks use.
#[test]
fn p1_status_db_path_is_the_same_for_a_dotdot_root() {
    let base = scratch("p1status");
    repo(&base, "x");
    ok(&run(&base, &["build", "--manifest", "--full", "x"]), "build");
    let plain = json(&run(&base, &["--root", "x", "--json", "status"]), "status x");
    let dotdot = json(
        &run(&base, &["--root", "x/../x", "--json", "status"]),
        "status x/../x",
    );
    assert_eq!(field(&plain, "generation_id"), field(&dotdot, "generation_id"));
    assert_eq!(
        field(&plain, "db_path"),
        field(&dotdot, "db_path"),
        "status names the store by a different string for a `..` spelling"
    );
}

/// A `..`-spelled build records the repo root it was given; a later plain query
/// must not be refused or marked stale for it.
#[test]
fn p1_a_dotdot_build_is_served_to_a_plain_query_and_back() {
    let base = scratch("p1roundtrip");
    repo(&base, "x");
    repo(&base, "y");
    ok(
        &run(&base, &["build", "--manifest", "--full", "y/../x"]),
        "build via ..",
    );
    let plain = run(&base, &["--root", "x", "--json", "search", "a"]);
    ok(&plain, "plain query over a `..`-built store");
    let dotdot = run(&base, &["--root", "x/../x", "--json", "search", "a"]);
    ok(&dotdot, "`..` query");
    assert_eq!(
        String::from_utf8_lossy(&plain.stdout),
        String::from_utf8_lossy(&dotdot.stdout)
    );
    let stale = json(&run(&base, &["--root", "x", "--json", "status"]), "status");
    assert_eq!(stale["is_fresh"], true, "{stale}");
}

/// The recorded repository root must be one string whatever the spelling.
#[test]
fn p1_the_recorded_repo_root_does_not_depend_on_the_spelling() {
    let base = scratch("p1recorded");
    let a = repo(&base, "a");
    let b = repo(&base, "b");
    repo(&base, "y");
    ok(&run(&base, &["build", "--full", "a"]), "build a");
    ok(&run(&base, &["build", "--full", "y/../b"]), "build b via ..");
    let open = |root: &Path| {
        let db = devmap_extract::paths::store_path(root);
        devmap_store::Store::open_read_only(&db)
            .unwrap()
            .latest_repo_root()
            .unwrap()
            .unwrap()
    };
    let plain = open(&a);
    let dotdot = open(&b);
    assert_eq!(
        plain.replace("/a", ""),
        dotdot.replace("/b", ""),
        "recorded roots differ in shape: {plain} vs {dotdot}"
    );
    assert!(!dotdot.contains(".."), "a `..` was stored as the repo root: {dotdot}");
}

// ---- Probe 2: symlinked parent ----------------------------------------------

/// The cwd reached through a symlink: `getcwd` is physical, so `.` anchors to
/// the canonical path, and must name the store a canonical absolute root names.
#[cfg(unix)]
#[test]
fn p2_dot_from_a_symlinked_cwd_is_the_canonical_identity() {
    let base = scratch("p2dot");
    let home = scratch("p2links");
    let real = repo(&base, "x");
    std::os::unix::fs::symlink(&base, home.join("lnk")).unwrap();
    let via_link = home.join("lnk").join("x");
    ok(&run(&via_link, &["build", "--manifest", "--full", "--root", "."]), "build .");
    let dot = json(&run(&via_link, &["--root", ".", "--json", "paths"]), "paths .");
    let canon = json(
        &run(&base, &["--json", "paths", real.to_str().unwrap()]),
        "paths canonical",
    );
    assert_eq!(field(&dot, "root"), field(&canon, "root"));
    assert_eq!(field(&dot, "db_path"), field(&canon, "db_path"));
    assert!(real.join(".devmap").is_dir(), "state must land in the real tree");
    assert!(!home.join("lnk/x/x").exists());
}

/// Two spellings of one repository (symlink-parent absolute vs canonical
/// absolute) must not make two stores. The anchor leaves absolute roots as
/// spelled, so a relative root from a symlinked cwd and an absolute root via
/// the link disagree about the string; the store must still be one.
#[cfg(unix)]
#[test]
fn p2_symlinked_absolute_and_canonical_absolute_roots_share_one_store() {
    let base = scratch("p2two");
    let home = scratch("p2twolinks");
    let real = repo(&base, "x");
    std::os::unix::fs::symlink(&base, home.join("lnk")).unwrap();
    let via_link = home.join("lnk").join("x");
    // The store refuses a link-spelled absolute root ("use its real path"), so
    // two spellings cannot make two stores: the link spelling makes none.
    let out = run(&home, &["build", "--manifest", "--full", via_link.to_str().unwrap()]);
    assert!(
        !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("real path"),
        "{}",
        text(&out)
    );
    assert!(
        !real.join(".devmap").exists(),
        "a refused build left state behind: {:?}",
        files(&real)
    );
    ok(
        &run(&home, &["build", "--manifest", "--full", real.to_str().unwrap()]),
        "canonical build",
    );
}

/// A store built via the link spelling must be queryable with the canonical
/// spelling that GitPulse passes (and the reverse), not refused as a different
/// repository.
#[cfg(unix)]
#[test]
fn p2_a_store_built_through_a_link_answers_the_canonical_spelling() {
    let base = scratch("p2q");
    let home = scratch("p2qlinks");
    let real = repo(&base, "x");
    std::os::unix::fs::symlink(&base, home.join("lnk")).unwrap();
    let via_link = home.join("lnk").join("x");
    ok(&run(&home, &["build", "--full", real.to_str().unwrap()]), "build canonical");
    // A read through the link spelling must be refused or agree with the
    // canonical answer, never report a different (empty) store.
    let by_link = run(&home, &["--root", via_link.to_str().unwrap(), "--json", "search", "a"]);
    let canon = run(&home, &["--root", real.to_str().unwrap(), "--json", "search", "a"]);
    ok(&canon, "canonical query");
    assert!(
        !by_link.status.success() || by_link.stdout == canon.stdout,
        "link spelling answered differently from canonical: {}",
        text(&by_link)
    );
    ok(&run(&via_link, &["--root", ".", "--json", "search", "a"]), "dot query");
}

// ---- Probe 3: serve ---------------------------------------------------------

fn socket(cwd: &Path, args: &[&str]) -> String {
    let out = run(cwd, args);
    ok(&out, &args.join(" "));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn p3_every_spelling_of_a_root_names_one_daemon_endpoint() {
    let base = scratch("p3");
    let real = repo(&base, "x");
    std::fs::create_dir_all(real.join("sub")).unwrap();
    repo(&base, "y");
    let want = socket(
        &base,
        &["serve", "--print-socket-path", real.to_str().unwrap()],
    );
    for (cwd, spelling) in [
        (base.clone(), "x"),
        (base.clone(), "./x/"),
        (base.clone(), "y/../x"),
        (base.join("y"), "../x"),
        (real.join("sub"), ".."),
        (real.clone(), "."),
    ] {
        assert_eq!(
            socket(&cwd, &["serve", "--print-socket-path", spelling]),
            want,
            "{spelling} from {}",
            cwd.display()
        );
    }
}

/// `--root` and the positional disagree: the daemon is rooted at the positional
/// while the store comes from `--root`. Both are anchored now, so the pair must
/// at least describe one repository, or be refused.
#[test]
fn p3_root_flag_and_positional_cannot_split_the_endpoint_from_the_store() {
    let base = scratch("p3split");
    repo(&base, "x");
    repo(&base, "y");
    let out = run(
        &base,
        &["--root", "x", "--json", "serve", "--print-socket-path", "y"],
    );
    let named_y = socket(&base, &["serve", "--print-socket-path", "y"]);
    let named_x = socket(&base, &["serve", "--print-socket-path", "x"]);
    let got = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !out.status.success() || (got.contains(&named_y) != got.contains(&named_x)),
        "ambiguous: {}",
        text(&out)
    );
    // The store belongs to --root (`x`) while the endpoint is `y`'s: that
    // combination binds repository y's tree to repository x's store.
    assert!(
        !out.status.success() || named_x == named_y,
        "`--root x serve y` accepted two repositories at once: {}",
        text(&out)
    );
}

// ---- Probe 4: commands not exercised ----------------------------------------

fn normalised(out: &Output, base: &Path, repo: &Path, name: &str) -> String {
    String::from_utf8_lossy(&out.stdout)
        .replace(repo.to_str().unwrap(), "<R>")
        .replace(base.to_str().unwrap(), "<B>")
        .replace(name, "<N>")
}

fn pair(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = scratch(tag);
    let rel = repo(&base, "rel");
    let abs = repo(&base, "abs");
    for dir in ["rel", "abs"] {
        std::fs::write(
            base.join(dir).join("routes.py"),
            "from flask import Flask\napp = Flask(__name__)\n@app.get('/h')\ndef h():\n    return 1\n",
        )
        .unwrap();
        ok(
            &run(&base, &["build", "--manifest", "--full", dir]),
            "build",
        );
    }
    (base, rel, abs)
}

fn no_doubling(base: &Path, rel: &Path, what: &str) {
    assert!(!rel.join("rel").exists(), "{what}: doubled directory {:?}", files(rel));
    assert!(!base.join("rel").join("rel").exists());
    let stray: Vec<_> = files(base)
        .into_iter()
        .filter(|f| !(f.starts_with("rel/") || f.starts_with("abs/")))
        .collect();
    assert!(stray.is_empty(), "{what}: wrote outside both repositories: {stray:?}");
}

#[test]
fn p4_html_export_map_html_agree_with_the_absolute_spelling() {
    let (base, rel, abs) = pair("p4write");
    let abs_s = abs.to_str().unwrap();
    for (cmd, extra) in [
        ("html", vec![]),
        ("export", vec![]),
        ("map-html", vec!["--force"]),
        ("map-html", vec!["--force", "--output", "out.html"]),
    ] {
        let mut a = vec!["--json", cmd, "rel"];
        a.extend(extra.iter().copied());
        let mut b = vec!["--json", cmd, abs_s];
        b.extend(extra.iter().copied());
        let r = run(&base, &a);
        let s = run(&base, &b);
        ok(&r, &format!("{cmd} rel"));
        ok(&s, &format!("{cmd} abs"));
        let strip = |o: &Output, repo: &Path, name: &str| {
            let mut v = normalised(o, &base, repo, name);
            // bytes / fingerprints may legitimately embed the repo name
            v.retain(|c| c != '\n');
            v
        };
        let (r_json, s_json): (serde_json::Value, serde_json::Value) = (
            serde_json::from_str(&strip(&r, &rel, "rel")).unwrap_or_default(),
            serde_json::from_str(&strip(&s, &abs, "abs")).unwrap_or_default(),
        );
        assert_eq!(r_json["output"], s_json["output"], "{cmd} {extra:?}: output path differs");
        assert_eq!(
            r_json["written"], s_json["written"],
            "{cmd} {extra:?}: written differs"
        );
    }
    no_doubling(&base, &rel, "write commands");
    let r: BTreeSet<_> = files(&rel)
        .into_iter()
        .map(|f| f.replace("rel", "<N>"))
        .collect();
    let s: BTreeSet<_> = files(&abs)
        .into_iter()
        .map(|f| f.replace("abs", "<N>"))
        .collect();
    assert_eq!(r, s, "relative and absolute left different files");
}

#[test]
fn p4_routes_shape_check_api_impact_agree_with_the_absolute_spelling() {
    let (base, rel, abs) = pair("p4read");
    let abs_s = abs.to_str().unwrap();
    for cmd in [
        vec!["routes"],
        vec!["shape-check"],
        vec!["api-impact", "/h"],
    ] {
        let with = |root: &str| -> Vec<String> {
            let mut v = vec!["--json".to_string()];
            v.extend(cmd.iter().map(|s| s.to_string()));
            v.push(root.to_string());
            v
        };
        let (ra, sa) = (with("rel"), with(abs_s));
        let r = run(&base, &ra.iter().map(String::as_str).collect::<Vec<_>>());
        let s = run(&base, &sa.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(
            r.status.code(),
            s.status.code(),
            "{cmd:?}: exit differs\nREL {}\nABS {}",
            text(&r),
            text(&s)
        );
        assert_eq!(
            normalised(&r, &base, &rel, "rel").replace("<N>", ""),
            normalised(&s, &base, &abs, "abs").replace("<N>", ""),
            "{cmd:?}: output differs"
        );
    }
    no_doubling(&base, &rel, "read commands");
}

/// From a different cwd than the repository's parent (`..` and sibling spellings).
#[test]
fn p4_a_sibling_relative_root_from_another_cwd_is_not_doubled() {
    let (base, rel, abs) = pair("p4cwd");
    let other = base.join("abs");
    for cmd in [
        vec!["html", "../rel"],
        vec!["export", "../rel"],
        vec!["map-html", "--force", "../rel"],
        vec!["routes", "../rel"],
        vec!["manifest", "../rel"],
    ] {
        let mut a = vec!["--json"];
        a.extend(cmd.iter().copied());
        ok(&run(&other, &a), &cmd.join(" "));
    }
    assert!(!rel.join("..").join("rel").join("rel").exists());
    assert!(!files(&rel).iter().any(|f| f.contains("rel/")), "{:?}", files(&rel));
    assert!(
        !files(&abs).iter().any(|f| f.starts_with("..") || f.contains("rel")),
        "the cwd repository was written to: {:?}",
        files(&abs)
    );
}

/// `integrate --project-root` is a root-taking argument that does not go through
/// `root_parser`: a relative spelling must name the same files as the absolute.
#[test]
fn p4_integrate_project_root_relative_matches_absolute() {
    let base = scratch("p4integrate");
    let rel = repo(&base, "rel");
    let abs = repo(&base, "abs");
    ok(&run(&base, &["build", "--manifest", "--full", "rel"]), "build rel");
    ok(&run(&base, &["build", "--manifest", "--full", "abs"]), "build abs");
    let r = run(
        &base,
        &["integrate", "claude", "--dry-run", "--project-root", "rel"],
    );
    let s = run(
        &base,
        &[
            "integrate",
            "claude",
            "--dry-run",
            "--project-root",
            abs.to_str().unwrap(),
        ],
    );
    assert_eq!(r.status.code(), s.status.code(), "REL {}\nABS {}", text(&r), text(&s));
    assert_eq!(
        normalised(&r, &base, &rel, "rel"),
        normalised(&s, &base, &abs, "abs"),
        "relative project root diverges from absolute"
    );
}

// ---- Probe 5: empty and odd args --------------------------------------------

#[test]
fn p5_an_empty_root_is_refused_by_every_root_taking_command() {
    let base = scratch("p5empty");
    repo(&base, "x");
    for args in [
        vec!["--root", "", "status"],
        vec!["build", ""],
        vec!["manifest", ""],
        vec!["freshness", ""],
        vec!["paths", ""],
        vec!["serve", "--print-socket-path", ""],
        vec!["routes", ""],
    ] {
        let out = run(&base, &args);
        assert!(
            !out.status.success(),
            "{args:?}: an empty root must be refused, not read as the cwd: {}",
            text(&out)
        );
    }
    assert!(files(&base).iter().all(|f| f == "x/a.rs"), "{:?}", files(&base));
}

#[test]
fn p5_dash_and_newline_roots_are_refused_and_create_nothing() {
    let base = scratch("p5odd");
    repo(&base, "x");
    for odd in ["-", "x\n", "x\r", " x", "x ", "x\0y"] {
        if odd.contains('\0') {
            continue;
        }
        for args in [
            vec!["build", "--manifest", "--full", odd],
            vec!["--root", odd, "build"],
            vec!["--root", odd, "--json", "status"],
            vec!["paths", odd],
        ] {
            let out = run(&base, &args);
            assert!(
                !out.status.success(),
                "{args:?}: refused spelling was accepted: {}",
                text(&out)
            );
        }
    }
    assert_eq!(
        files(&base),
        BTreeSet::from(["x/a.rs".to_string()]),
        "a refused root created files"
    );
}

/// A bare `-` positional must be refused as a nonexistent directory and must
/// not be taken as stdin or an option.
#[test]
fn p5_a_dash_root_names_itself_in_the_refusal() {
    let base = scratch("p5dash");
    let out = run(&base, &["build", "--", "-"]);
    assert!(!out.status.success(), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("No such file") || err.contains("not a directory"), "{err}");
}

/// Validation of `.` is unchanged: still accepted in a plain directory.
#[test]
fn p5_dot_is_still_a_valid_root() {
    let base = scratch("p5dot");
    let x = repo(&base, "x");
    ok(&run(&x, &["build", "--manifest", "--full", "."]), "build .");
    ok(&run(&x, &["--root", ".", "--json", "status"]), "status .");
    assert!(x.join(".devmap").is_dir());
}

/// A relative root whose anchor would exceed what the OS accepts must still be
/// a clean refusal rather than a panic.
#[test]
fn p5_an_overlong_relative_root_is_a_refusal_not_a_panic() {
    let base = scratch("p5long");
    let long = "d".repeat(300);
    let out = run(&base, &["build", &long]);
    assert!(!out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("panicked"),
        "{}",
        text(&out)
    );
}

// ---- Probe 6: Windows-style separators (characterization) -------------------

/// On Unix a backslash is an ordinary file-name byte, so `x\sub` is one
/// component and is refused as missing. Characterizes the current behaviour:
/// it is not translated to a separator and nothing is created.
#[cfg(unix)]
#[test]
fn p6_backslash_is_a_filename_byte_on_unix() {
    let base = scratch("p6");
    let x = repo(&base, "x");
    std::fs::create_dir_all(x.join("sub")).unwrap();
    let out = run(&base, &["build", "--manifest", "--full", "x\\sub"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!x.join("sub/.devmap").exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("x\\sub"));
    let drive = run(&base, &["paths", "C:\\repo"]);
    assert!(!drive.status.success(), "{}", text(&drive));
    assert!(
        !String::from_utf8_lossy(&drive.stderr).contains("panicked"),
        "{}",
        text(&drive)
    );
}
