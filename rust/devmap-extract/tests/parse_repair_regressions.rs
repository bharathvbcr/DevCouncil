//! Regression tests for:
//! - Error-localized same-width parse repairs in TypeScript, Rust, and Shell
//! - Rust string line-continuation backslash whitespace stripping
//! - JS/TS factory-returned object literal methods and `export default { ... }`

use devmap_extract::extract_file;
use devmap_extract::model::{ParseOutcome, SymbolKind};

fn is_clean(path: &str, source: &str) -> bool {
    matches!(
        extract_file(path, source).parse_outcome,
        ParseOutcome::Clean
    )
}

#[test]
fn typescript_import_types_tuple_keyword_labels_and_unique_comparison_parse_clean() {
    let source = "\
type Mod = typeof import('./store');
type Store = import('./store').InterfaceStore;
type Pair = [verb: string, object: string, number: number];

export function check(a: number, unique: { count: number }): boolean {
  return a < unique.count;
}
";
    let ext = extract_file("src/types.ts", source);
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "outcome: {:?}",
        ext.parse_outcome
    );
    assert!(
        ext.references.iter().any(|r| r.name == "InterfaceStore"),
        "import('...').Member must preserve the Member type reference: {:?}",
        ext.references
    );
    assert!(
        ext.symbols.iter().any(|s| s.name == "check"),
        "missing check symbol: {:?}",
        ext.symbols
    );
}

#[test]
fn typescript_real_syntax_error_stays_partial() {
    let ext = extract_file(
        "src/broken.ts",
        "export function broken( { return [1, )]; }\n",
    );
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Partial { .. }),
        "real TS syntax error must stay Partial: {:?}",
        ext.parse_outcome
    );
}

#[test]
fn rust_2024_unsafe_extern_raw_borrow_and_cfg_struct_pattern_parse_clean() {
    let source = "\
unsafe extern \"C\" {
    fn native_hook(x: i32) -> i32;
}

struct Entry {
    path: String,
    #[cfg(unix)]
    ino: u64,
}

pub fn inspect(raw: &str, e: Entry) -> usize {
    let r = &raw;
    let Entry {
        path,
        #[cfg(unix)]
        ino,
    } = e;
    #[cfg(unix)]
    let _ = ino;
    r.len() + path.len()
}
";
    let ext = extract_file("src/fs_entry.rs", source);
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "outcome: {:?}",
        ext.parse_outcome
    );
    assert!(
        ext.symbols.iter().any(|s| s.name == "native_hook"),
        "missing extern fn symbol: {:?}",
        ext.symbols
    );
    assert!(
        ext.symbols.iter().any(|s| s.name == "inspect"),
        "missing inspect symbol: {:?}",
        ext.symbols
    );
}

#[test]
fn rust_real_raw_const_pointer_and_real_syntax_error_are_not_corrupted() {
    let raw_ptr = "\
pub fn ptr(x: &u8) -> *const u8 {
    &raw const *x
}
";
    // Must not falsely rewrite `&raw const`; whether tree-sitter-rust accepts
    // it natively or reports Partial, it must never break real syntax checks.
    let _ = extract_file("src/ptr.rs", raw_ptr);

    let broken = "pub fn broken( { let x = ; }\n";
    assert!(
        !is_clean("src/broken.rs", broken),
        "real Rust syntax error must stay Partial"
    );
}

#[test]
fn shell_quoted_param_expansion_with_parens_parses_clean_and_preserves_cmd_sub() {
    let source = "\
#!/usr/bin/env bash
set -euo pipefail
pin=\"abcdef1234567890\"
echo \"commit${pin:+ (${pin:0:12})}\"
out=\"$(printf 'hi')\"
echo \"$out\"
";
    let ext = extract_file("scripts/session-start.sh", source);
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "outcome: {:?}",
        ext.parse_outcome
    );
    assert!(
        ext.calls.iter().any(|c| c.callee_name == "printf"),
        "command substitution $(printf ...) must still be parsed: {:?}",
        ext.calls
    );

    let broken = "if true; then\n  echo hi\n";
    assert!(
        !is_clean("scripts/broken.sh", broken),
        "unclosed Shell if-block must stay Partial"
    );
}

#[test]
fn rust_string_line_continuation_strips_leading_whitespace() {
    let source = "\
pub fn msg() -> &'static str {
    \"first line \\
        second line\\
\t\tthird line\"
}
";
    let ext = extract_file("src/msg.rs", source);
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "outcome: {:?}",
        ext.parse_outcome
    );
    assert!(
        ext.literals
            .iter()
            .any(|l| l.value == "first line second linethird line"),
        "expected collapsed line-continuation literal, got: {:?}",
        ext.literals
    );
}

#[test]
fn js_ts_factory_object_methods_and_default_export_methods_are_extracted_with_call_attribution() {
    let source = "\
function helper() {}

function createInterfaceStore() {
  return {
    applyPreset: (id: string) => {
      helper();
      return id;
    },
    reset() {
      helper();
    },
  };
}

export const interfaceStore = createInterfaceStore();

export function createRepoStore() {
  const store = {
    trustRepo: (path: string) => helper(),
  };
  return store;
}

export default {
  boot: () => helper(),
};
";
    let ext = extract_file("src/store.ts", source);
    assert!(matches!(ext.parse_outcome, ParseOutcome::Clean));

    for (qname, expected_parent) in [
        (
            "src/store.ts::createInterfaceStore.applyPreset",
            "src/store.ts::createInterfaceStore",
        ),
        (
            "src/store.ts::createInterfaceStore.reset",
            "src/store.ts::createInterfaceStore",
        ),
        (
            "src/store.ts::createRepoStore.trustRepo",
            "src/store.ts::createRepoStore",
        ),
        ("src/store.ts::boot", "src/store.ts"),
    ] {
        let sym = ext
            .symbols
            .iter()
            .find(|s| s.qualified_name == qname)
            .unwrap_or_else(|| panic!("missing symbol {qname} in {:?}", ext.symbols));
        assert_eq!(sym.kind, SymbolKind::Method, "{qname}");
        assert_eq!(
            sym.parent_symbol.as_deref(),
            Some(expected_parent),
            "{qname}"
        );
    }

    for expected_caller in [
        "src/store.ts::createInterfaceStore.applyPreset",
        "src/store.ts::createInterfaceStore.reset",
        "src/store.ts::createRepoStore.trustRepo",
        "src/store.ts::boot",
    ] {
        assert!(
            ext.calls.iter().any(|c| {
                c.callee_name == "helper" && c.caller_symbol.as_deref() == Some(expected_caller)
            }),
            "call to helper() must be attributed to {expected_caller}: {:?}",
            ext.calls
        );
    }
}
