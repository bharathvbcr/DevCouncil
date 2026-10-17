//! A member of an object built by spread, `Object.assign`, a plain alias or a
//! forwarding property resolves to the member's own declaration.
//!
//! `export const debugLogger = { ...coreDebugLogger, ... }` then
//! `debugLogger.warn()` landed in the ledger as `uninferred_receiver`: the
//! member table of a const bound to an object literal held only what the
//! literal spells, so a key reached through a spread, an `Object.assign`
//! source or an alias had no owner. Properties apply in source order and the
//! last write wins, so a later source or an explicit key outranks an earlier
//! spread; a source the extraction cannot name stops the walk for every key it
//! could define, and a cycle abstains.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::{ResolutionResult, Resolver};

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

fn targets(result: &ResolutionResult, from: &str) -> Vec<String> {
    let mut found: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.source_symbol == from && edge.edge_kind == EdgeKind::Calls)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    found.sort();
    found
}

const CORE: &str = "\
export const core = {
  info(): void {},
  warn(): void {},
};
export const other = {
  info(): void {},
};
export function clearLogs(): void {}
";

const CORE_INFO: &str = "src/core.ts::core.info";
const OTHER_INFO: &str = "src/core.ts::other.info";

/// Resolve `user` beside [`CORE`] and return what `src/user.ts::run` calls.
fn run_calls(user: &str) -> Vec<String> {
    let result = resolve(&[("src/core.ts", CORE), ("src/user.ts", user)]);
    targets(&result, "src/user.ts::run")
}

fn binds(user: &str, expected: &str) {
    let found = run_calls(user);
    assert!(
        found.iter().any(|t| t == expected),
        "expected an edge to {expected}, got {found:?}\n{user}"
    );
}

fn binds_nothing(user: &str) {
    let found = run_calls(user);
    assert!(found.is_empty(), "expected no edge, got {found:?}\n{user}");
}

#[test]
fn a_spread_member_binds_to_the_imported_source() {
    binds(
        "import { core } from './core';\nconst w = { ...core };\nexport function run() { w.info(); }\n",
        CORE_INFO,
    );
}

#[test]
fn an_object_assign_source_binds_to_the_imported_source() {
    binds(
        "import { core } from './core';\nconst w = Object.assign({}, core);\nexport function run() { w.info(); }\n",
        CORE_INFO,
    );
}

#[test]
fn an_exported_alias_binds_from_another_file() {
    let alias = "import { core } from './core';\nexport const aliased = core;\n";
    let consumer =
        "import { aliased } from './alias';\nexport function run() { aliased.info(); }\n";
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/alias.ts", alias),
        ("src/user.ts", consumer),
    ]);
    assert_eq!(targets(&result, "src/user.ts::run"), vec![CORE_INFO]);
}

#[test]
fn a_forwarding_property_binds_to_the_forwarded_function() {
    let holder = "import { clearLogs } from './core';\nexport const x = { clear: clearLogs };\n";
    let consumer = "import { x } from './holder';\nexport function run() { x.clear(); }\n";
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/holder.ts", holder),
        ("src/user.ts", consumer),
    ]);
    assert_eq!(
        targets(&result, "src/user.ts::run"),
        vec!["src/core.ts::clearLogs"]
    );
}

#[test]
fn a_shorthand_property_binds_to_the_function_it_names() {
    let holder = "import { clearLogs } from './core';\nexport const x = { clearLogs };\n";
    let consumer = "import { x } from './holder';\nexport function run() { x.clearLogs(); }\n";
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/holder.ts", holder),
        ("src/user.ts", consumer),
    ]);
    assert_eq!(
        targets(&result, "src/user.ts::run"),
        vec!["src/core.ts::clearLogs"]
    );
}

#[test]
fn a_spread_in_another_file_than_the_receiver_binds() {
    let holder = "import { core } from './core';\nexport const debugLogger = { ...core, extra() {} };\n";
    let consumer = "import { debugLogger } from './holder';\nexport function run() { debugLogger.warn(); }\n";
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/holder.ts", holder),
        ("src/user.ts", consumer),
    ]);
    assert_eq!(
        targets(&result, "src/user.ts::run"),
        vec!["src/core.ts::core.warn"]
    );
}

#[test]
fn the_later_of_two_spreads_wins() {
    let found = run_calls(
        "import { core, other } from './core';\nconst w = { ...core, ...other };\nexport function run() { w.info(); }\n",
    );
    assert_eq!(found, vec![OTHER_INFO]);
    let found = run_calls(
        "import { core, other } from './core';\nconst w = { ...other, ...core };\nexport function run() { w.info(); }\n",
    );
    assert_eq!(found, vec![CORE_INFO]);
}

#[test]
fn a_key_a_later_spread_lacks_falls_back_to_the_earlier_one() {
    binds(
        "import { core, other } from './core';\nconst w = { ...core, ...other };\nexport function run() { w.warn(); }\n",
        "src/core.ts::core.warn",
    );
}

#[test]
fn an_explicit_key_after_a_spread_beats_it() {
    let source = "\
const core = { info() {} };
export const w = { ...core, info() {} };
";
    let consumer = "import { w } from './w';\nexport function run() { w.info(); }\n";
    let result = resolve(&[("src/w.ts", source), ("src/user.ts", consumer)]);
    assert_eq!(targets(&result, "src/user.ts::run"), vec!["src/w.ts::w.info"]);
}

#[test]
fn a_spread_after_an_explicit_key_beats_it() {
    let source = "\
const core = { info() {} };
export const w = { info() {}, ...core };
";
    let consumer = "import { w } from './w';\nexport function run() { w.info(); }\n";
    let result = resolve(&[("src/w.ts", source), ("src/user.ts", consumer)]);
    assert_eq!(
        targets(&result, "src/user.ts::run"),
        vec!["src/w.ts::core.info"]
    );
}

#[test]
fn an_unknown_later_spread_abstains_for_every_key() {
    binds_nothing(
        "import { core } from './core';\ndeclare function make(): object;\nconst w = { ...core, ...make() };\nexport function run() { w.info(); w.warn(); }\n",
    );
}

#[test]
fn an_unknown_earlier_spread_abstains_only_for_keys_the_known_one_lacks() {
    let found = run_calls(
        "import { core } from './core';\ndeclare function make(): object;\nconst w = { ...make(), ...core };\nexport function run() { w.info(); w.zap(); }\n",
    );
    assert_eq!(found, vec![CORE_INFO]);
}

#[test]
fn a_cycle_terminates_and_abstains() {
    let source = "\
export const a = { ...b };
export const b = { ...a };
";
    let consumer = "import { a } from './cyc';\nexport function run() { a.info(); }\n";
    let result = resolve(&[("src/cyc.ts", source), ("src/user.ts", consumer)]);
    assert!(targets(&result, "src/user.ts::run").is_empty());
}

#[test]
fn a_namespace_import_spread_binds_to_the_module_function() {
    let ns = "export function info(): void {}\n";
    let user = "import * as core from './ns';\nconst w = { ...core };\nexport function run() { w.info(); }\n";
    let result = resolve(&[("src/ns.ts", ns), ("src/user.ts", user)]);
    assert_eq!(targets(&result, "src/user.ts::run"), vec!["src/ns.ts::info"]);
}

#[test]
fn a_commonjs_require_spread_binds_to_the_module_function() {
    let ns = "export function info(): void {}\n";
    let user = "const core = require('./ns');\nconst w = { ...core };\nexport function run() { w.info(); }\n";
    let result = resolve(&[("src/ns.ts", ns), ("src/user.ts", user)]);
    assert_eq!(targets(&result, "src/user.ts::run"), vec!["src/ns.ts::info"]);
}

#[test]
fn a_default_import_spread_never_binds_a_wrong_member() {
    let found = run_calls(
        "import core from './core';\nconst w = { ...core };\nexport function run() { w.info(); }\n",
    );
    assert!(
        !found.iter().any(|t| t == OTHER_INFO),
        "a default import is no named export, got {found:?}"
    );
}

#[test]
fn object_assign_into_an_existing_const_adds_the_source() {
    binds(
        "import { core } from './core';\nconst w = { extra() {} };\nObject.assign(w, core);\nexport function run() { w.info(); }\n",
        CORE_INFO,
    );
}

#[test]
fn object_assign_inside_a_function_makes_the_target_unknown() {
    binds_nothing(
        "import { core } from './core';\nconst w = {};\nfunction patch() { Object.assign(w, core); }\nexport function run() { w.info(); }\n",
    );
}

#[test]
fn a_nested_spread_binds() {
    binds(
        "import { core } from './core';\nconst w = { ...{ ...core } };\nexport function run() { w.info(); }\n",
        CORE_INFO,
    );
}

#[test]
fn a_spread_of_a_call_result_abstains() {
    binds_nothing(
        "declare function make(): object;\nconst w = { ...make() };\nexport function run() { w.info(); }\n",
    );
}

#[test]
fn a_tsx_file_binds() {
    let result = resolve(&[
        ("src/core.ts", CORE),
        (
            "src/View.tsx",
            "import { core } from './core';\nconst w = { ...core };\nexport function run() { w.info(); return <div />; }\n",
        ),
    ]);
    assert_eq!(targets(&result, "src/View.tsx::run"), vec![CORE_INFO]);
}

#[test]
fn one_identifier_imported_from_two_modules_binds_per_file() {
    let a = "import { core } from './core';\nconst w = { ...core };\nexport function run() { w.info(); }\n";
    let b = "import { core } from './core2';\nconst w = { ...core };\nexport function run() { w.info(); }\n";
    let core2 = "export const core = { info() {} };\n";
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/core2.ts", core2),
        ("src/a.ts", a),
        ("src/b.ts", b),
    ]);
    assert_eq!(targets(&result, "src/a.ts::run"), vec![CORE_INFO]);
    assert_eq!(targets(&result, "src/b.ts::run"), vec!["src/core2.ts::core.info"]);
}

#[test]
fn a_local_that_shadows_the_object_is_not_the_object() {
    binds_nothing(
        "import { core } from './core';\nconst w = { ...core };\nexport function run(w: { info(): void }) { w.info(); }\n",
    );
}
