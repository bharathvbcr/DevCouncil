//! `devmap literals --sky-time` is a query, not an unknown flag.
//!
//! The report: a JS file passes the CSS custom property `--sky-time` to
//! `setProperty`, and `devmap literals sky-time` found nothing, because the
//! match was a prefix, and `devmap literals --sky-time` was refused by clap as
//! an unexpected argument, so the only spelling that worked was
//! `devmap literals -- --sky-time`. Both are covered here through the binary,
//! together with the flags that must still be flags beside such a query.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const AMBIENCE: &str = "function seed(pal) {
  document.documentElement.dataset.timeOfDay = pal.mode;
  document.documentElement.style.setProperty('--sky-time', pal.mode);
}
";

fn devmap(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .env("DEVMAP_AUTOSPAWN", "0")
        .args(["--progress", "never"])
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

#[track_caller]
fn ok(root: &Path, args: &[&str]) -> String {
    let output = devmap(root, args);
    assert!(
        output.status.success(),
        "devmap {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[track_caller]
fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let stdout = ok(root, &full);
    serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("{full:?}: {error}: {stdout}"))
}

fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-literals-hyphen-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/ambience.js"), AMBIENCE).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.invalid"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "fixture"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    root.canonicalize().unwrap()
}

#[track_caller]
fn assert_one_sky_time_site(label: &str, answer: &serde_json::Value, exact: bool) {
    assert_eq!(answer["exact"], exact, "{label}: {answer}");
    assert_eq!(answer["total"], 1, "{label}: {answer}");
    let site = &answer["items"][0];
    assert_eq!(site["file_path"], "src/ambience.js", "{label}: {answer}");
    assert_eq!(site["line"], 3, "{label}: {answer}");
    assert_eq!(site["value"], "--sky-time", "{label}: {answer}");
}

#[test]
fn a_query_that_starts_with_dashes_is_taken_as_the_query() {
    let root = fixture();
    ok(&root, &["build"]);

    // The text answer names the site and says how it matched.
    let text = ok(&root, &["literals", "--sky-time"]);
    assert!(
        text.contains("literals substring \"--sky-time\"") && text.contains("src/ambience.js:3"),
        "{text}"
    );

    // The query matches inside a value.
    let inner = json(&root, &["literals", "sky-time"]);
    assert_one_sky_time_site("sky-time", &inner, false);
    assert_eq!(inner["query"], "sky-time");

    // A hyphen-leading query, with the real flags on either side of it.
    for args in [
        vec!["literals", "--sky-time"],
        vec!["literals", "-b", "100", "--sky-time"],
        vec!["literals", "--sky-time", "--budget", "100"],
        vec!["literals", "--", "--sky-time"],
    ] {
        let answer = json(&root, &args);
        assert_one_sky_time_site(&format!("{args:?}"), &answer, false);
        assert_eq!(answer["query"], "--sky-time", "{args:?}: {answer}");
    }
    for args in [
        vec!["literals", "--exact", "--sky-time"],
        vec!["literals", "--sky-time", "--exact"],
    ] {
        let answer = json(&root, &args);
        assert_one_sky_time_site(&format!("{args:?}"), &answer, true);
        assert_eq!(answer["query"], "--sky-time", "{args:?}: {answer}");
    }
    // A global flag after the query is still the global flag.
    let trailing = ok(&root, &["literals", "--sky-time", "--json"]);
    let trailing: serde_json::Value = serde_json::from_str(&trailing)
        .unwrap_or_else(|error| panic!("--json after the query: {error}: {trailing}"));
    assert_one_sky_time_site("--json after the query", &trailing, false);

    // `--exact` is still equality: the inner piece is not the whole value.
    let partial = json(&root, &["literals", "--exact", "sky-time"]);
    assert_eq!(partial["total"], 0, "{partial}");

    // A real flag is never swallowed as the query.
    let help = ok(&root, &["literals", "--help"]);
    assert!(help.contains("--exact"), "--help prints help: {help}");
    for args in [vec!["literals"], vec!["literals", "--exact"]] {
        let output = devmap(&root, &args);
        assert!(
            !output.status.success(),
            "{args:?} has no query and must be refused:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("<QUERY>"), "{args:?}: {stderr}");
    }

    let _ = std::fs::remove_dir_all(&root);
}
