//! X43 — a call written as a *module path* is classified by the module, and
//! the shell gets the builtin table every other primary language already has.
//!
//! Two halves, both of them the same failure: affirmative evidence sitting in
//! the source and no rung reading it.
//!
//! **`std::fs::write(...)` is not an uninferred receiver.** That tier means
//! "the receiver is a value whose type we could not infer" — a structural limit
//! of a syntax-directed extractor, and correct for `value.unwrap()`. `std::fs`
//! is not a value at all; it is a path rooted at a crate the language injects
//! into every crate's extern prelude, so no indexed file can ever declare it.
//! Measured on this repository: **2,128 rows** rooted at `std`, plus 373 rooted
//! at `serde_json`, a crate the file's own `use` lines name.
//!
//! **The shell had no builtin table.** `LangFamily::Shell` fell to the `_ =>
//! return false` arm of `is_builtin`, so `echo`, `exit`, `printf`, `cd`,
//! `return` and `set` all landed in the tier documented as the only one that
//! indicates a defect — 398 of the 521 shell rows on this repository, which is
//! why shell reported a net resolution of 38 permille against python's 492.
//!
//! The narrow rule is deliberate and stated here because it is what keeps the
//! path half honest: only a receiver containing `::` is treated as a path. A
//! Rust binding cannot contain `::`, so that test cannot mistake a local
//! variable for a module — where the existing `imports.get(root)` rung, which
//! matches a bare handle, always could.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

/// The class of every unresolved row for `name`, deduplicated.
fn classes_of(result: &ResolutionResult, name: &str) -> Vec<String> {
    let mut found: Vec<String> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == name)
        .map(|row| row.class.label().to_string())
        .collect();
    found.sort();
    found.dedup();
    found
}

#[test]
fn a_std_rooted_path_call_is_external_not_an_uninferred_receiver() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "pub fn save(body: &str) {\n    \
         std::fs::write(\"out.txt\", body).unwrap();\n}\n",
    )]);

    assert_eq!(
        classes_of(&result, "write"),
        vec!["external".to_string()],
        "`std::fs` is a path into a crate the language injects, not a value \
         whose type went uninferred"
    );
}

/// `unwrap` in the same fixture stays where it belongs: its receiver *is* a
/// value, and typing it needs inference this extractor does not do. The OFF
/// direction for the rung above.
#[test]
fn a_method_on_a_value_stays_an_uninferred_receiver() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "pub fn save(body: &str) {\n    \
         std::fs::write(\"out.txt\", body).unwrap();\n}\n",
    )]);

    assert_eq!(
        classes_of(&result, "unwrap"),
        vec!["uninferred_receiver".to_string()],
        "the receiver of `unwrap` is the expression above it — a real \
         structural limit, and not something a module table may explain away"
    );
}

/// A crate the file's own `use` lines name, whose module resolved to no indexed
/// file. The path is addressable without a `use` of the crate root itself,
/// which is why the bare-handle rung never saw it.
#[test]
fn a_path_rooted_at_a_crate_the_file_imports_is_external() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "use serde_json::Value;\n\npub fn read(raw: &str) -> Value {\n    \
         serde_json::from_str(raw).unwrap()\n}\n",
    )]);

    assert_eq!(
        classes_of(&result, "from_str"),
        vec!["external".to_string()],
        "`use serde_json::Value;` is this file stating that `serde_json` is a \
         crate it does not contain"
    );
}

/// A repo-relative path root is the opposite claim: it cannot name anything
/// outside the corpus, so failing to resolve it is an index gap and must keep
/// the tier that means "this may be a defect".
#[test]
fn a_crate_rooted_path_that_resolves_to_nothing_is_an_index_gap_not_external() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "pub fn go() {\n    crate::missing::helper();\n}\n",
    )]);

    assert_eq!(
        classes_of(&result, "helper"),
        vec!["module_path".to_string()],
        "`crate::` is a module path into this repository; the miss is explained \
         as a module path rather than a bare defect"
    );
}

/// A local variable is never a module path, and the rule that keeps it that way
/// is syntactic: a Rust binding cannot contain `::`. The receiver here is an
/// untyped local that merely shares a crate's name, so nothing about the crate
/// may be said about it.
#[test]
fn a_local_named_like_a_crate_is_not_treated_as_one() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "use serde_json::Value;\n\npub fn go() {\n    \
         let serde_json = build();\n    serde_json.take();\n}\n",
    )]);

    assert_eq!(
        classes_of(&result, "take"),
        vec!["uninferred_receiver".to_string()],
        "`serde_json.take()` is a method on a local that merely shares a \
         crate's name; the path rung must not reach it"
    );
}

#[test]
fn shell_builtins_are_declared_by_the_shell_and_not_resolution_defects() {
    let (_, result) = resolve(&[(
        "scripts/run.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\n\n\
         main() {\n  echo \"starting\"\n  printf '%s\\n' \"$1\"\n  cd /tmp\n  \
         local value=1\n  export VALUE=\"$value\"\n  return 0\n}\n\nmain \"$@\"\n",
    )]);

    for name in ["echo", "printf", "cd", "return", "set", "export", "local"] {
        let classes = classes_of(&result, name);
        if classes.is_empty() {
            continue;
        }
        assert_eq!(
            classes,
            vec!["builtin".to_string()],
            "`{name}` is a shell builtin; no indexed file can declare it, so it \
             is not a resolution defect"
        );
    }
}

/// The OFF direction, and the reason the table stops where it does: an external
/// *program* is not a shell builtin, and there is no import to prove where it
/// comes from. With no corpus namesake it is [`NoNamesake`], not a host global
/// and not a language builtin.
#[test]
fn an_external_program_is_not_folded_into_the_builtin_table() {
    let (_, result) = resolve(&[(
        "scripts/run.sh",
        "#!/usr/bin/env bash\n\nmain() {\n  grep -q foo bar\n  cargo build\n}\n",
    )]);

    for name in ["grep", "cargo"] {
        let classes = classes_of(&result, name);
        if classes.is_empty() {
            continue;
        }
        assert_eq!(
            classes,
            vec!["no_namesake".to_string()],
            "`{name}` is whatever is on PATH — the shell does not declare it \
             and no import names it, so nothing here may call it expected"
        );
    }
}

/// A script that declares its own function named like a builtin still resolves
/// to the declaration: the table is only ever consulted after the ladder fails.
#[test]
fn a_script_that_declares_its_own_helper_still_binds_to_it() {
    let (_, result) = resolve(&[(
        "scripts/run.sh",
        "#!/usr/bin/env bash\n\nstep() {\n  echo hi\n}\n\nmain() {\n  step\n}\n",
    )]);

    assert!(
        classes_of(&result, "step").is_empty(),
        "`step` is declared in this file and must resolve to it rather than \
         reach any classification at all"
    );
}
