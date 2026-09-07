//! A message the kernel prints is one sentence, not a sentence with a hole in it.
//!
//! Read back from the release binary, `trace X X` reported:
//!
//! ```text
//! stopped at depth 3 after                          visiting 44 nodes
//! ```
//!
//! and `preview` of an unparsed buffer:
//!
//! ```text
//! no delta is reported,                      because an unparsed file yields no symbols
//! ```
//!
//! Both were `format!` literals re-wrapped across lines without the `\`
//! continuation that eats the next line's indentation, so the indentation
//! became part of the message. Fixing the two would fix two; this scans every
//! crate's sources for the shape and names each one, so the class is closed
//! and stays closed.
//!
//! The shape: inside a string literal, a word or punctuation, then ten or
//! more spaces, then a lowercase letter or an opening parenthesis. Literals
//! that carry `\n` are source fixtures whose indentation is the point, and a
//! run that begins at the literal's start is column alignment for a report
//! line; neither is a message with a hole in it.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && path
                .file_name()
                .is_some_and(|name| name != file!().rsplit('/').next().unwrap_or(""))
        {
            out.push(path);
        }
    }
}

/// Whether `line` holds a string literal with a run of spaces inside a
/// sentence. Hand-rolled rather than a regex so this crate's test does not
/// depend on one.
fn has_a_hole(line: &str) -> bool {
    if !line.contains('"') || line.contains("\\n") || line.trim_start().starts_with("//") {
        return false;
    }
    let bytes = line.as_bytes();
    let mut in_literal = false;
    let mut previous: Option<u8> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' && previous != Some(b'\\') {
            in_literal = !in_literal;
            previous = Some(byte);
            index += 1;
            continue;
        }
        if in_literal && byte == b' ' {
            let run_start = index;
            while index < bytes.len() && bytes[index] == b' ' {
                index += 1;
            }
            let run = index - run_start;
            let before_is_text =
                previous.is_some_and(|b| b.is_ascii_alphanumeric() || b":;,.)`".contains(&b));
            let after_is_text = bytes
                .get(index)
                .is_some_and(|b| b.is_ascii_lowercase() || *b == b'(');
            if run >= 10 && before_is_text && after_is_text {
                return true;
            }
            previous = Some(b' ');
            continue;
        }
        previous = Some(byte);
        index += 1;
    }
    false
}

#[test]
fn no_message_literal_in_the_kernel_carries_a_run_of_spaces() {
    let root = workspace_root().join("crates");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    assert!(
        sources.len() > 100,
        "only {} sources found under {}; the scan is not looking at the kernel",
        sources.len(),
        root.display()
    );
    let mut holes = Vec::new();
    for path in &sources {
        let text = std::fs::read_to_string(path).expect("readable source");
        for (number, line) in text.lines().enumerate() {
            if has_a_hole(line) {
                holes.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(&root).unwrap_or(path).display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        holes.is_empty(),
        "{} message literal(s) carry a run of spaces inside a sentence — a `format!` \
         re-wrapped without `\\` continuations; the reader gets the indentation as \
         part of the message:\n  {}",
        holes.len(),
        holes.join("\n  ")
    );
}

#[test]
fn the_shape_is_what_the_scan_looks_for() {
    assert!(has_a_hole(
        r#"reason: format!("stopped at {limits} after                 visiting {n} nodes")"#
    ));
    assert!(has_a_hole(
        r#""no delta is reported,                      because an unparsed""#
    ));
    // A fixture with newlines keeps its indentation.
    assert!(!has_a_hole(
        r#""class C:\n    def m(self):\n                    return 1\n""#
    ));
    // Column alignment at the start of a report line.
    assert!(!has_a_hole(
        r#"println!("      {label} took {:.0}ms", seconds)"#
    ));
    assert!(!has_a_hole(
        r#"println!("  map answer cost:        {} tokens", n)"#
    ));
    // Code outside a literal, and a comment that quotes the shape.
    assert!(!has_a_hole(
        "let x = 1;                    // aligned comment"
    ));
    assert!(!has_a_hole(
        r#"    /// reported "after                          visiting 44 nodes""#
    ));
}
