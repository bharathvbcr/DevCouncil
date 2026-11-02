//! The soak's own output names the file it churns and why it chose it, so a
//! run on a corpus with no Python says that it measured the TypeScript
//! fallback rather than looking like every other run.

#![cfg(unix)]

mod common;

use std::fs;

use common::{fixture, soak};

#[test]
fn the_output_names_the_churn_file_and_why_it_was_chosen() {
    // The stand-in build fails at once: the line is printed before any build.
    let (root, stub, _) = fixture("churn-py", "exit 1");
    let out = soak(&root, &stub).output().expect("the soak runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("soak churn file: pkg/a.py (first .py file"),
        "{text}"
    );
    let _ = fs::remove_dir_all(root.parent().expect("base"));

    let (root, stub, _) = fixture("churn-ts", "exit 1");
    fs::remove_file(root.join("pkg/a.py")).expect("remove");
    fs::write(root.join("pkg/b.ts"), "export {};\n").expect("ts");
    let out = soak(&root, &stub).output().expect("the soak runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("soak churn file: pkg/b.ts (TypeScript fallback: no .py file"),
        "{text}"
    );
    let _ = fs::remove_dir_all(root.parent().expect("base"));
}
