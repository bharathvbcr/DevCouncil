#![cfg(feature = "parse")]

use devmap_extract::model::{ParseOutcome, ReferenceKind};
use devmap_extract::treesitter::extract_treesitter_with_budget;
use std::time::Duration;

/// Nesting at the 200k scale an adversarial 1 MiB source reaches. At this
/// depth the extraction is cut short by its budget long before any type walker
/// runs, so these fixtures prove only that the file gets an outcome.
const DEEP: usize = 200_000;

/// Nesting that the walk finishes inside the budget, so the self-recursive
/// type walker under test does run — and, unbounded, overflows the child's
/// 256 KiB stack. A fixture refused for time at this depth fails, because it
/// would prove nothing about the walker.
const REACHED: usize = 20_000;

fn check_child(case: &str) {
    crate::run_child_test(
        "nested_source_limits::nested_source_child",
        &[("DEVMAP_NESTED_SOURCE_CASE", case)],
        Duration::from_secs(120),
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

macro_rules! deep_cases {
    ($($reached:ident, $deep:ident => $case:literal;)*) => {$(
        #[test]
        fn $reached() {
            check_child(&format!("{}:{REACHED}", $case));
        }
        #[test]
        fn $deep() {
            check_child(&format!("{}:{DEEP}", $case));
        }
    )*};
}

deep_cases! {
    rust_parenthesized_operands_cannot_exhaust_the_native_stack,
    rust_parenthesized_operands_at_200k_return_a_file_outcome => "rust_parens";
    rust_reference_operands_cannot_exhaust_the_native_stack,
    rust_reference_operands_at_200k_return_a_file_outcome => "rust_refs";
    swift_array_parameter_types_cannot_exhaust_the_native_stack,
    swift_array_parameter_types_at_200k_return_a_file_outcome => "swift_array_parameter";
    swift_array_property_types_cannot_exhaust_the_native_stack,
    swift_array_property_types_at_200k_return_a_file_outcome => "swift_array_property";
    rust_let_initializers_cannot_exhaust_the_native_stack,
    rust_let_initializers_at_200k_return_a_file_outcome => "rust_let_initializer";
}

/// The deep fixtures, each a source whose nesting reaches one self-recursive
/// walker, `depth` levels deep.
fn deep_source(name: &str, depth: usize) -> (&'static str, String) {
    let open = |token: &str| token.repeat(depth);
    match name {
        // `rust_typed_operand`, on the left operand of every binary expression.
        "rust_parens" => (
            "rust",
            format!("fn f() {{ let _ = {}a{} + b; }}\n", open("("), open(")")),
        ),
        "rust_refs" => (
            "rust",
            format!("fn f() {{ let _ = {}a + b; }}\n", open("& ")),
        ),
        // `swiftscope::normalize`, reached from a parameter and from an
        // annotated `let`; the call in each body makes the scope's bindings
        // computed.
        "swift_array_parameter" => (
            "swift",
            format!("func f(_ a: {}Int{}) {{ a.run() }}\n", open("["), open("]")),
        ),
        "swift_array_property" => (
            "swift",
            format!(
                "func f() {{ let a: {}Int{} = make()\n a.run() }}\n",
                open("["),
                open("]")
            ),
        ),
        // `node_contains`, asked whether a use sits in the initializer of a
        // `let` binding the same name.
        "rust_let_initializer" => (
            "rust",
            format!(
                "fn g() -> u8 {{ 1 }}\nfn f() {{ let g = {}g(){}; }}\n",
                open("("),
                open(")")
            ),
        ),
        _ => panic!("unknown deep fixture {name}"),
    }
}

fn run_deep_case(name: &str, depth: usize) {
    let (language, source) = deep_source(name, depth);
    let budget = if depth == DEEP {
        // The production budget: the bound a daemon actually runs under.
        Duration::from_secs(5)
    } else {
        Duration::from_secs(60)
    };
    let result = extract_treesitter_with_budget("nested", language, &source, budget);
    println!("{name}@{depth}: {:?}", result.parse_outcome);
    assert!(
        !result.symbols.is_empty(),
        "extraction must return an explicit file outcome"
    );
    if depth == REACHED {
        if let ParseOutcome::Failed { reason } = &result.parse_outcome {
            panic!("{name}@{depth} was refused, so the walker under test never ran: {reason}");
        }
        // A capped walker says it stopped: the file is not silently read as if
        // the deep type had stated nothing.
        let capped = match name {
            "rust_parens" | "rust_refs" => Some("rust_typed_operand stopped at 16 levels"),
            "swift_array_parameter" | "swift_array_property" => {
                Some("swiftscope::normalize stopped at 16 levels")
            }
            _ => None,
        };
        if let Some(note) = capped {
            assert!(
                result.diagnostics.iter().any(|line| line.starts_with(note)),
                "{name}@{depth} must record that the walker stopped: {:?}",
                result.diagnostics
            );
        }
    }
}

#[test]
#[ignore = "child process driven by the bounded regression tests above"]
fn nested_source_child() {
    let case = std::env::var("DEVMAP_NESTED_SOURCE_CASE").unwrap();
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            if let Some((name, depth)) = case.split_once(':') {
                run_deep_case(name, depth.parse().expect("a nesting depth"));
                return;
            }
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
