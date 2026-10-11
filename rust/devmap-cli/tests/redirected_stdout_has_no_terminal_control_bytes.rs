//! Output that is not a terminal must not carry bytes a terminal acts on.
//!
//! The control-character escaper ran only on the interactive path. Piped or
//! `--json` output went straight to stdout, so a file named with an OSC title
//! sequence, a screen clear and a right-to-left override reached whatever read
//! the pipe — a pager, a terminal multiplexer, an agent transcript — verbatim.
//! JSON text is serde-escaped for C0 but not for bidirectional controls.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-stdout-escape-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Every character a terminal or a bidi-aware renderer acts on, other than the
/// newline and tab that structure text output.
fn hostile(c: char) -> bool {
    (c != '\n' && c != '\t' && c.is_control())
        || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

#[test]
fn piped_and_json_output_escape_a_hostile_file_name() {
    let root = scratch("names");
    let name = "evil\u{1b}]0;owned\u{7}\u{1b}[2J\u{202e}gnp.py";
    std::fs::write(root.join(name), "def hostile_target():\n    return 1\n").unwrap();
    let db = root.join("store.sqlite").to_string_lossy().into_owned();
    let root_s = root.to_string_lossy().into_owned();
    let build = Command::new(devmap())
        .args(["--db", &db, "--progress", "never", "build", &root_s])
        .output()
        .expect("spawn devmap build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut saw_the_file = 0;
    for args in [
        vec!["search", "hostile_target"],
        vec!["search", "hostile_target", "--json"],
        vec!["explore", "hostile_target"],
        vec!["explore", "hostile_target", "--json"],
        vec!["status", "--json"],
    ] {
        let output = Command::new(devmap())
            .args(["--db", &db, "--progress", "never"])
            .args(&args)
            .output()
            .expect("spawn devmap");
        let text = String::from_utf8_lossy(&output.stdout);
        if let Some(bad) = text.chars().find(|&c| hostile(c)) {
            panic!("{args:?}: stdout carries {bad:?}:\n{text:?}");
        }
        if text.contains("evil") {
            saw_the_file += 1;
        }
    }
    assert!(
        saw_the_file >= 2,
        "the hostile name never reached stdout; the test proved nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}
