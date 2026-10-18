//! Closing probes 5 and 7 against `lane/deps-outbound` @ 41a0d9be, at the
//! `devmap deps` surface. Failing tests are findings; nothing is fixed here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

fn corpus(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-deps-close-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("lib.py"),
        "RETRY = 2\n\n\ndef helper():\n    return 1\n\n\ndef worker():\n    helper()\n    return RETRY\n",
    )
    .unwrap();
    std::fs::write(root.join("callers.py"), "from lib import worker\n\n\ndef caller():\n    return worker()\n")
        .unwrap();
    let built = run(&root, &["build", "."]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    root
}

/// Probe 5c. An invalid `--min-rung` is refused loudly, for symbol and file
/// queries, and an empty value is not read as "no floor".
#[test]
fn p5_an_invalid_min_rung_is_refused() {
    let root = corpus("rung");
    let mut report = String::new();
    for target in ["lib.py", "worker"] {
        for rung in ["bogus", "", "Deterministic", "lsp"] {
            let out = run(&root, &["deps", "--min-rung", rung, "--", target]);
            if out.status.success() {
                report.push_str(&format!("{target} --min-rung {rung:?} accepted\n"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(report.is_empty(), "{report}");
}

/// Probe 7. What the terminal prints for the note carries no raw control
/// character from the query: one line, `note: ...`, and no byte below 0x20
/// other than the line feeds that end lines. Severity: low.
#[test]
fn p7_the_printed_note_is_one_escaped_line() {
    let root = corpus("hygiene");
    let out = run(&root, &["deps", "--", "RETRY\n"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let _ = std::fs::remove_dir_all(&root);
    let text = format!("{stdout}{stderr}");
    let line = text
        .lines()
        .find(|l| l.starts_with("note:"))
        .unwrap_or_else(|| panic!("the probe did not reach the note:\n{text}"));
    assert!(
        line.contains("depends on nothing"),
        "the note is split across lines by a raw newline in the query: {text:?}"
    );
}
