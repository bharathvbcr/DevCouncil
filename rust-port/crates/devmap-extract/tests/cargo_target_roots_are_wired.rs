//! A file Cargo compiles as a root is not a file nothing needs.
//!
//! Found by measuring this repository rather than by reading code. After import
//! extraction landed, seventeen Rust files were still reported as unwired
//! candidates and eleven of them were Cargo target roots: five `src/lib.rs`
//! crate roots, five `examples/*.rs`, one `build.rs`. Each was a delete-this
//! suggestion for a file named in a `Cargo.toml` — and no import will ever
//! point at one, because a target root is where the module tree *starts*.
//!
//! The first attempt at this fix marked them `ScriptEntry`, which is the
//! annotation `main.rs` already carries. That broke
//! `test_runtime_entry_points_are_exempt_without_exempting_their_file`
//! immediately: every file-level wiring kind exempts every symbol in the file
//! from the dead-code verdict, so an unused helper in `src/bin/tool.rs` became
//! exempt because its file had a `main`. `TargetRoot` is a claim about the
//! file's wiring and nothing inside it, and these tests hold both halves apart.

use devmap_extract::extract_file;
use devmap_extract::model::WiringKind;
use devmap_extract::wiring::{rust_path_declares_main, rust_target_root_reason};

fn target_root_reason(path: &str, source: &str) -> Option<String> {
    extract_file(path, source)
        .wiring
        .iter()
        .find(|annotation| {
            annotation.kind == WiringKind::TargetRoot && annotation.target_symbol == path
        })
        .map(|annotation| annotation.details.clone())
}

fn file_level_kinds(path: &str, source: &str) -> Vec<WiringKind> {
    let mut kinds: Vec<WiringKind> = extract_file(path, source)
        .wiring
        .iter()
        .filter(|annotation| annotation.target_symbol == path)
        .map(|annotation| annotation.kind)
        .collect();
    kinds.sort_by_key(|kind| format!("{kind:?}"));
    kinds.dedup();
    kinds
}

#[test]
fn every_cargo_target_root_is_annotated_with_its_reason() {
    for (path, expected) in [
        ("crates/app/src/lib.rs", "Cargo library crate root"),
        ("crates/app/src/main.rs", "Cargo binary crate root"),
        ("crates/app/build.rs", "Cargo build script"),
        ("crates/app/src/bin/tool.rs", "Cargo binary target"),
        ("crates/app/examples/demo.rs", "Cargo example target"),
        ("crates/app/benches/bench.rs", "Cargo benchmark target"),
    ] {
        assert_eq!(
            target_root_reason(path, "pub fn f() {}\n").as_deref(),
            Some(expected),
            "{path}"
        );
    }
}

/// An ordinary module is not a target root. Without this the rule would exempt
/// every Rust file in the repository and the finding would be gone rather than
/// corrected.
#[test]
fn an_ordinary_module_is_not_a_target_root() {
    for path in [
        "crates/app/src/parser.rs",
        "crates/app/src/parser/mod.rs",
        // A `lib.rs` nested inside a module directory is an ordinary module —
        // Cargo compiles a crate root only at the crate's own root.
        "crates/app/src/vendor/lib.rs",
    ] {
        assert_eq!(
            target_root_reason(path, "pub fn f() {}\n"),
            None,
            "{path} is an ordinary module and can genuinely be stranded"
        );
    }
}

/// The whole reason `TargetRoot` is its own kind.
///
/// `src/bin/tool.rs` is a target root and must not become an unwired candidate.
/// It must *also* not exempt `tool_helper` from the dead-code verdict, which is
/// what marking it `ScriptEntry` did.
#[test]
fn a_binary_target_root_does_not_exempt_its_own_symbols() {
    let kinds = file_level_kinds(
        "crates/app/src/bin/tool.rs",
        "fn tool_helper() {}\n\nfn main() {}\n",
    );
    assert!(
        kinds.contains(&WiringKind::TargetRoot),
        "the file must be marked as a target root: {kinds:?}"
    );
    assert!(
        !kinds.contains(&WiringKind::ScriptEntry),
        "`ScriptEntry` exempts every symbol in the file from the dead-code \
         verdict, and an unused helper in a binary target is exactly as dead as \
         one anywhere else: {kinds:?}"
    );
}

/// `src/main.rs` keeps the `ScriptEntry` it has always had, and gains the new
/// annotation beside it. Nothing about its existing behaviour moves.
#[test]
fn a_binary_crate_root_keeps_its_script_entry() {
    let kinds = file_level_kinds("crates/app/src/main.rs", "fn main() {}\n");
    assert!(kinds.contains(&WiringKind::ScriptEntry), "{kinds:?}");
    assert!(kinds.contains(&WiringKind::TargetRoot), "{kinds:?}");
}

/// The two rules answer different questions and must not be collapsed.
///
/// The first attempt made `rust_path_declares_main` delegate to
/// `rust_target_root_reason`, which said `src/lib.rs` declares `main`. A
/// library crate root has no `fn main` at all, so a `main` written in one is an
/// ordinary function and a real dead-code candidate — which two existing tests
/// caught on the first run.
#[test]
fn a_library_crate_root_is_a_target_root_but_declares_no_main() {
    assert_eq!(
        rust_target_root_reason("crates/app/src/lib.rs"),
        Some("Cargo library crate root")
    );
    assert!(
        !rust_path_declares_main("crates/app/src/lib.rs"),
        "`main` in a library file has no toolchain caller and is ordinary code"
    );
}

/// A build script's `fn main` *is* toolchain-invoked, so both rules accept it.
#[test]
fn a_build_script_declares_main_and_is_a_target_root() {
    assert_eq!(
        rust_target_root_reason("crates/app/build.rs"),
        Some("Cargo build script")
    );
    assert!(rust_path_declares_main("crates/app/build.rs"));
}

/// Integration tests are target *targets* but not target roots here: they are
/// already exempt as test files, and seeding the reachability walk from them
/// would make every helper a test reaches look live.
#[test]
fn an_integration_test_is_not_a_target_root_but_declares_main() {
    assert_eq!(rust_target_root_reason("crates/app/tests/it.rs"), None);
    assert!(rust_path_declares_main("crates/app/tests/it.rs"));
}
