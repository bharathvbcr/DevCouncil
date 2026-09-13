#![cfg(feature = "parse")]

use devmap_extract::model::{ParseOutcome, ReferenceKind};
use devmap_extract::subprocess::{run_bounded, Bounds};
use devmap_extract::treesitter::extract_treesitter_with_budget;
use std::process::Command;
use std::time::Duration;

fn check_child(case: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--ignored", "--exact", "nested_source_child", "--nocapture"])
        .env("DEVMAP_NESTED_SOURCE_CASE", case);
    let result = run_bounded(
        &mut child,
        Bounds {
            deadline: Duration::from_secs(5),
            stdout_cap: 16384,
            stderr_cap: 16384,
        },
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{case} crashed or failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn javascript_heritage_cannot_exhaust_the_native_stack() {
    check_child("javascript");
}
#[test]
fn python_heritage_cannot_exhaust_the_native_stack() {
    check_child("python");
}
#[test]
fn kotlin_unary_calls_cannot_exhaust_the_native_stack() {
    check_child("kotlin");
}
#[test]
fn nested_callable_names_cannot_exhaust_the_native_stack() {
    check_child("scopes");
}

#[test]
#[ignore = "child process driven by the bounded regression tests above"]
fn nested_source_child() {
    let case = std::env::var("DEVMAP_NESTED_SOURCE_CASE").unwrap();
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let (language, source) = match case.as_str() {
                "javascript" => (
                    "javascript",
                    format!(
                        "class W extends {}Base{} {{}}",
                        "(".repeat(3000),
                        ")".repeat(3000)
                    ),
                ),
                "python" => (
                    "python",
                    format!(
                        "class W({}Base{}):\n    pass\n",
                        "(".repeat(3000),
                        ")".repeat(3000)
                    ),
                ),
                "kotlin" => ("kotlin", format!("fun f() {{ {}g() }}", "!".repeat(3000))),
                "scopes" => (
                    "javascript",
                    format!("{}g();{}", "function f(){".repeat(400), "}".repeat(400)),
                ),
                _ => panic!("unknown fixture"),
            };
            let result = extract_treesitter_with_budget(
                "nested",
                language,
                &source,
                Duration::from_millis(250),
            );
            assert!(
                !result.symbols.is_empty(),
                "extraction must return an explicit file outcome"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn bounded_helpers_preserve_ordinary_heritage_and_nested_scopes() {
    let result = extract_treesitter_with_budget(
        "ordinary.js",
        "javascript",
        "class W extends Base {} function outer(){ function inner(){ call(); } }",
        Duration::from_secs(2),
    );
    assert!(matches!(result.parse_outcome, ParseOutcome::Clean));
    assert!(result
        .references
        .iter()
        .any(|reference| reference.name == "Base" && reference.kind == ReferenceKind::Heritage));
    let call = result
        .calls
        .iter()
        .find(|call| call.callee_name == "call")
        .unwrap();
    assert!(call
        .caller_symbol
        .as_deref()
        .unwrap()
        .ends_with("::outer.inner"));
}
