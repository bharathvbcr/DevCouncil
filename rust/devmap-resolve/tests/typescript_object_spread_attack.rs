//! Adversarial probes for the object-spread rung (2a'') and its extractor
//! facts. Each test states the sound answer; a failing test is a BREAK.
//!
//! Every probe resolves `src/user.ts` beside [`CORE`] and reads the Calls
//! edges whose source file is `src/user.ts`, so module-level calls (which have
//! no enclosing function) are visible too.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::{ResolutionResult, Resolver};
use std::time::Instant;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

/// `(source symbol, target symbol)` of every Calls edge written in `file`.
fn calls_in(result: &ResolutionResult, file: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = result
        .edges
        .iter()
        .filter(|e| e.source_file == file && e.edge_kind == EdgeKind::Calls)
        .map(|e| (e.source_symbol.clone(), e.target_symbol.clone()))
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

fn user_targets(user: &str) -> Vec<String> {
    let result = resolve(&[("src/core.ts", CORE), ("src/user.ts", user)]);
    calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect()
}

fn never_binds(user: &str, wrong: &[&str]) {
    let found = user_targets(user);
    for w in wrong {
        assert!(
            !found.iter().any(|t| t == w),
            "bound the unsound target {w}; edges {found:?}\n{user}"
        );
    }
}

fn binds_exactly(user: &str, expected: &[&str]) {
    let found = user_targets(user);
    assert_eq!(found, expected, "\n{user}");
}

const HEAD: &str = "import { core, other, clearLogs } from './core';\n";

// ---------------------------------------------------------------- precision

#[test]
fn computed_key_variable_after_spread_abstains() {
    never_binds(
        &format!("{HEAD}const k = 'info';\nconst w = {{ ...core, [k]: clearLogs }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn computed_key_string_literal_after_spread_never_binds_the_spread() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core, ['info']: clearLogs }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_getter_after_a_spread_never_binds_the_spread_member() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core, get info() {{ return () => {{}}; }} }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_getter_is_not_the_callee_of_a_call() {
    // `w.info()` calls the function the getter *returns*, not the getter.
    let found = user_targets(&format!(
        "{HEAD}const w = {{ ...core, get info() {{ return () => {{}}; }} }};\nexport function run() {{ w.info(); }}\n"
    ));
    assert!(
        !found.iter().any(|t| t == "src/user.ts::w.info"),
        "an accessor was bound as the call target: {found:?}"
    );
}

#[test]
fn a_setter_after_a_spread_never_binds_the_spread_member() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core, set info(v: number) {{}} }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn method_shorthand_beside_a_spread_wins() {
    binds_exactly(
        &format!(
            "{HEAD}const w = {{ ...core, info() {{}} }};\nexport function run() {{ w.info(); }}\n"
        ),
        &["src/user.ts::w.info"],
    );
}

#[test]
fn proto_key_is_never_a_wrong_bind() {
    let found = user_targets(&format!(
        "{HEAD}const w = {{ __proto__: other, ...core }};\nexport function run() {{ w.warn(); }}\n"
    ));
    assert!(
        found.iter().all(|t| t == "src/core.ts::core.warn"),
        "{found:?}"
    );
    // `__proto__: core` supplies keys through the chain: never a different object's.
    never_binds(
        &format!("{HEAD}const w = {{ __proto__: core }};\nexport function run() {{ w.info(); }}\n"),
        &[OTHER_INFO],
    );
}

#[test]
fn spread_of_this_after_a_known_spread_abstains() {
    never_binds(
        &format!(
            "{HEAD}const w = {{ ...core, ...this }};\nexport function run() {{ w.info(); }}\n"
        ),
        &[CORE_INFO],
    );
}

#[test]
fn spread_of_arguments_after_a_known_spread_abstains() {
    never_binds(
        &format!(
            "{HEAD}const w = {{ ...core, ...arguments }};\nexport function run() {{ w.info(); }}\n"
        ),
        &[CORE_INFO],
    );
}

#[test]
fn spread_of_an_array_literal_abstains() {
    never_binds(
        &format!("{HEAD}const w = {{ ...[core] }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn an_array_alias_is_no_object() {
    never_binds(
        &format!("{HEAD}const a = [core];\nconst w = {{ ...a }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn object_assign_through_a_function_parameter_taints_nothing_it_cannot_see() {
    // `patch(w)` mutates `w` through a parameter; the module-level write the
    // extractor sees is spelled `Object.assign(t, other)` and names `t`.
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nfunction patch(t: object) {{ Object.assign(t, other); }}\npatch(w);\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn object_assign_after_a_module_level_use_is_not_in_force_at_the_use() {
    // `w.info()` runs before `Object.assign(w, other)`: it reaches core.info,
    // never other.info.
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nw.info();\nObject.assign(w, other);\n"),
        &[OTHER_INFO],
    );
}

#[test]
fn a_conditional_object_assign_abstains() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nif (Math.random() > 0.5) Object.assign(w, other);\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO, OTHER_INFO],
    );
}

#[test]
fn a_short_circuit_object_assign_abstains() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nMath.random() > 0.5 && Object.assign(w, other);\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO, OTHER_INFO],
    );
}

#[test]
fn a_try_block_object_assign_abstains() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\ntry {{ Object.assign(w, other); }} catch (e) {{}}\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO, OTHER_INFO],
    );
}

#[test]
fn object_assign_inside_an_iife_abstains() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\n(() => {{ Object.assign(w, other); }})();\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO, OTHER_INFO],
    );
}

#[test]
fn object_assign_inside_a_class_static_block_abstains() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nclass K {{ static {{ Object.assign(w, other); }} }}\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO, OTHER_INFO],
    );
}

#[test]
fn let_is_reassignable_and_abstains() {
    never_binds(
        &format!("{HEAD}let w = {{ ...core }};\nw = {{ ...other }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn var_is_reassignable_and_abstains() {
    never_binds(
        &format!("{HEAD}var w = {{ ...core }};\nw = {{ ...other }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_function_local_const_shadows_the_module_object() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\ndeclare function make(): {{ info(): void }};\nexport function run() {{ const w = make(); w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_block_local_const_shadows_the_module_object() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\ndeclare function make(): {{ info(): void }};\nexport function run(flag: boolean) {{ if (flag) {{ const w = make(); w.info(); }} }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_nested_arrow_parameter_shadows_the_module_object() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nexport function run(items: {{ info(): void }}[]) {{ items.forEach((w) => w.info()); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_for_of_binding_shadows_the_module_object() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nexport function run(items: {{ info(): void }}[]) {{ for (const w of items) {{ w.info(); }} }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_catch_parameter_shadows_the_module_object() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nexport function run() {{ try {{ }} catch (w: any) {{ w.info(); }} }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_destructured_parameter_shadows_the_module_object() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nexport function run({{ w }}: {{ w: {{ info(): void }} }}) {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_class_method_parameter_shadows_the_module_object() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nexport class K {{ go(w: {{ info(): void }}) {{ w.info(); }} }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_block_scoped_const_is_not_the_module_object() {
    // `{ const w = ...core }` is block scope; the module-level `w` the call
    // reads is the import, whose object is `other`.
    let holder = "import { other } from './core';\nexport const w = { ...other };\n";
    let user = format!(
        "import {{ core }} from './core';\nimport {{ w }} from './holder';\n{{ const w = {{ ...core }}; }}\nexport function run() {{ w.info(); }}\n"
    );
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/holder.ts", holder),
        ("src/user.ts", &user),
    ]);
    let found: Vec<String> = calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert!(
        !found.iter().any(|t| t == CORE_INFO),
        "a block-scoped const was read as the module's object: {found:?}"
    );
}

#[test]
fn delete_removes_the_member() {
    never_binds(
        &format!("{HEAD}const w: any = {{ ...core }};\ndelete w.info;\nexport function run() {{ w.info?.(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_member_assignment_replaces_the_member() {
    never_binds(
        &format!("{HEAD}const w: any = {{ ...core }};\nw.info = clearLogs;\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_computed_member_assignment_replaces_the_member() {
    never_binds(
        &format!("{HEAD}const w: any = {{ ...core }};\nw['info'] = clearLogs;\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_member_assignment_inside_a_function_replaces_the_member() {
    never_binds(
        &format!("{HEAD}const w: any = {{ ...core }};\nexport function patch() {{ w.info = clearLogs; }}\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn an_assignment_through_an_alias_replaces_the_member() {
    never_binds(
        &format!("{HEAD}const w: any = {{ ...core }};\nconst a = w;\na.info = clearLogs;\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn define_property_replaces_the_member() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nObject.defineProperty(w, 'info', {{ value: clearLogs }});\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn define_properties_replaces_the_member() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nObject.defineProperties(w, {{ info: {{ value: clearLogs }} }});\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn set_prototype_of_does_not_shadow_an_own_key_but_must_not_invent_one() {
    // `warn` is not an own key of `w`; a prototype swap can supply it.
    never_binds(
        &format!("{HEAD}const w = {{ ...other }};\nObject.setPrototypeOf(w, core);\nexport function run() {{ w.warn(); }}\n"),
        &["src/core.ts::other.warn"],
    );
}

#[test]
fn reflect_set_replaces_the_member() {
    never_binds(
        &format!("{HEAD}const w = {{ ...core }};\nReflect.set(w, 'info', clearLogs);\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn object_freeze_of_a_spread_is_never_a_wrong_bind() {
    never_binds(
        &format!("{HEAD}const w = Object.freeze({{ ...other, ...core }});\nexport function run() {{ w.info(); }}\n"),
        &[OTHER_INFO],
    );
}

#[test]
fn an_as_cast_spread_binds() {
    binds_exactly(
        &format!("{HEAD}interface Logger {{ info(): void }}\nconst w = {{ ...core }} as Logger;\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn a_satisfies_spread_binds() {
    binds_exactly(
        &format!("{HEAD}interface Logger {{ info(): void }}\nconst w = {{ ...core }} satisfies Logger;\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn an_angle_bracket_assertion_spread_binds() {
    binds_exactly(
        &format!("{HEAD}interface Logger {{ info(): void }}\nconst w = <Logger>{{ ...core }};\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn an_optional_chained_call_binds() {
    binds_exactly(
        &format!("{HEAD}const w = {{ ...core }};\nexport function run() {{ w?.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn an_alias_chain_within_the_cap_binds() {
    binds_exactly(
        &format!("{HEAD}const a1 = core;\nconst a2 = a1;\nconst a3 = a2;\nexport function run() {{ a3.info(); }}\n"),
        &[CORE_INFO],
    );
}

#[test]
fn an_alias_chain_beyond_the_cap_abstains() {
    let mut body = String::from(HEAD);
    body.push_str("const a1 = core;\n");
    for i in 2..=14 {
        body.push_str(&format!("const a{i} = a{};\n", i - 1));
    }
    body.push_str("export function run() { a14.info(); }\n");
    binds_exactly(&body, &[]);
}

#[test]
fn an_alias_chain_beyond_the_cap_with_a_later_spread_never_partially_binds() {
    // Past the hop cap the walk gives up: the *earlier* spread must not answer.
    let mut body = String::from(HEAD);
    body.push_str("const a1 = { ...other };\n");
    for i in 2..=14 {
        body.push_str(&format!("const a{i} = {{ ...a{}, ...core }};\n", i - 1));
    }
    body.push_str("export function run() { a14.warn(); }\n");
    let found = user_targets(&body);
    assert!(
        !found.iter().any(|t| t == "src/core.ts::other.warn"),
        "{found:?}"
    );
}

#[test]
fn a_default_import_spread_never_binds_a_same_named_module_function() {
    // `info` the named export is a different function from the default
    // object's `info` member.
    let m = "export function info(): void {}\nexport default { info(): void {} };\n";
    let user = "import lg from './m';\nconst w = { ...lg };\nexport function run() { w.info(); }\n";
    let result = resolve(&[("src/m.ts", m), ("src/user.ts", user)]);
    let found: Vec<String> = calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert!(
        !found.iter().any(|t| t == "src/m.ts::info"),
        "bound the named export for a default object's member: {found:?}"
    );
}

#[test]
fn a_default_import_alias_never_binds_a_same_named_module_function() {
    let m =
        "export function info(): void {}\nconst obj = { info(): void {} };\nexport default obj;\n";
    let user = "import lg from './m';\nconst w = lg;\nexport function run() { w.info(); }\n";
    let result = resolve(&[("src/m.ts", m), ("src/user.ts", user)]);
    let found: Vec<String> = calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert!(
        !found.iter().any(|t| t == "src/m.ts::info"),
        "bound the named export for a default object's member: {found:?}"
    );
}

#[test]
fn an_export_star_collision_through_a_barrel_abstains() {
    let a = "export const core = { info(): void {} };\n";
    let b = "export const core = { info(): void {} };\n";
    let barrel = "export * from './a';\nexport * from './b';\n";
    let user = "import { core } from './barrel';\nconst w = { ...core };\nexport function run() { w.info(); }\n";
    let result = resolve(&[
        ("src/a.ts", a),
        ("src/b.ts", b),
        ("src/barrel.ts", barrel),
        ("src/user.ts", user),
    ]);
    let found: Vec<String> = calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert!(
        found.is_empty(),
        "an ambiguous `export *` name was bound: {found:?}"
    );
}

#[test]
fn an_object_prototype_method_never_binds_an_in_repo_namesake() {
    let other_file = "export function toString(): string { return ''; }\nexport class K { hasOwnProperty(): boolean { return true; } }\nexport function valueOf(): number { return 1; }\n";
    let user = format!(
        "{HEAD}const w = {{ ...core }};\nexport function run() {{ w.toString(); w.hasOwnProperty('x'); w.valueOf(); }}\n"
    );
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/ns.ts", other_file),
        ("src/user.ts", &user),
    ]);
    let found: Vec<String> = calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert!(
        found.is_empty(),
        "a prototype method bound in-repo: {found:?}"
    );
}

#[test]
fn a_prototype_named_member_of_the_spread_source_binds_the_source() {
    let core = "export const core = { toString(): string { return ''; } };\n";
    let user = "import { core } from './core';\nconst w = { ...core };\nexport function run() { w.toString(); }\n";
    let result = resolve(&[("src/core.ts", core), ("src/user.ts", user)]);
    let found: Vec<String> = calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(found, vec!["src/core.ts::core.toString"]);
}

// -------------------------------------------------------- abstain discipline

#[test]
fn an_unknown_earlier_spread_never_lets_a_global_namesake_answer_an_undefined_key() {
    let zap = "export function zap(): void {}\n";
    let user = format!(
        "{HEAD}declare function make(): object;\nconst w = {{ ...make(), ...core }};\nexport function run() {{ w.zap(); w.info(); }}\n"
    );
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/zap.ts", zap),
        ("src/user.ts", &user),
    ]);
    let found: Vec<String> = calls_in(&result, "src/user.ts")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(found, vec![CORE_INFO.to_string()]);
}

#[test]
fn a_key_nothing_defines_never_binds_a_global_namesake() {
    let nothing = "export function nothing(): void {}\n";
    let user =
        format!("{HEAD}const w = {{ ...core }};\nexport function run() {{ w.nothing(); }}\n");
    let result = resolve(&[
        ("src/core.ts", CORE),
        ("src/nothing.ts", nothing),
        ("src/user.ts", &user),
    ]);
    assert!(calls_in(&result, "src/user.ts").is_empty());
}

#[test]
fn an_unreadable_mutation_after_a_known_spread_abstains_for_unanswered_keys_only() {
    // `Object.assign(w, make())` may define anything: info is no longer certain.
    never_binds(
        &format!("{HEAD}declare function make(): object;\nconst w = {{ ...core }};\nObject.assign(w, make());\nexport function run() {{ w.info(); }}\n"),
        &[CORE_INFO],
    );
}

// ------------------------------------------------------------ cache staleness

/// A payload cached by ab05b9e6 carries no `js_objects` (the field is skipped
/// when empty). It is stamped with the same `extract-v95` identity this build
/// stamps, so the cache serves it back and the spread fix silently does
/// nothing for every unchanged file.
#[test]
fn a_payload_cached_before_js_objects_existed_is_not_servable() {
    let user = format!("{HEAD}const w = {{ ...core }};\nexport function run() {{ w.info(); }}\n");
    let fresh: Vec<Extraction> = vec![
        extract_file("src/core.ts", CORE),
        extract_file("src/user.ts", &user),
    ];
    let mut stale = fresh.clone();
    for ext in &mut stale {
        let mut json: serde_json::Value = serde_json::to_value(&*ext).unwrap();
        json.as_object_mut().unwrap().remove("js_objects");
        *ext = serde_json::from_value(json).unwrap();
    }
    assert!(stale.iter().all(|e| e.js_objects.is_empty()));
    assert!(fresh.iter().any(|e| !e.js_objects.is_empty()));

    let run = |exts: &[Extraction]| {
        let mut r = Resolver::new();
        r.index_extractions(exts);
        let result = r.resolve_all(exts).unwrap();
        calls_in(&result, "src/user.ts")
    };
    let fresh_edges = run(&fresh);
    let stale_edges = run(&stale);
    // What the resolver does with the stale row is a difference, not an error:
    // the guarantee the cache must give is that such a row is never reused.
    // The identity a v95 row carries today is the one ab05b9e6 stamped.
    let (_, analyzer) = devmap_extract::cache::current_payload_identity("typescript");
    assert_ne!(
        fresh_edges, stale_edges,
        "(sanity) the stale payload resolves differently from a fresh one"
    );
    assert!(
        !analyzer.ends_with(":extract-v95"),
        "this build still stamps {analyzer}: a v95 row cached by ab05b9e6 (no js_objects) is served as current, \
         so the spread rung never fires on it"
    );
}

// ------------------------------------------------------------ other languages

#[test]
fn jsobject_facts_exist_only_for_js_ts_family_files() {
    let ts = "const core = { a() {} };\nconst w = { ...core };\n";
    let cases: &[(&str, bool)] = &[
        ("a.ts", true),
        ("a.tsx", true),
        ("a.js", true),
        ("a.jsx", true),
        ("a.mjs", true),
        ("a.cjs", true),
        ("a.mts", true),
        ("a.cts", true),
        ("a.py", false),
        ("a.rb", false),
        ("a.go", false),
    ];
    for (path, expected) in cases {
        let ext = extract_file(path, ts);
        assert_eq!(
            !ext.js_objects.is_empty(),
            *expected,
            "{path}: language {} js_objects {}",
            ext.language,
            ext.js_objects.len()
        );
    }
}

#[test]
fn a_vue_script_block_spread_binds_like_a_ts_file() {
    let vue = "<template><div/></template>\n<script lang=\"ts\">\nimport { core } from './core';\nconst w = { ...core };\nexport function run() { w.info(); }\n</script>\n";
    let result = resolve(&[("src/core.ts", CORE), ("src/View.vue", vue)]);
    let found: Vec<String> = calls_in(&result, "src/View.vue")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(
        found,
        vec![CORE_INFO.to_string()],
        "embedded script behaviour"
    );
}

#[test]
fn a_svelte_script_block_spread_binds_like_a_ts_file() {
    let svelte = "<script lang=\"ts\">\nimport { core } from './core';\nconst w = { ...core };\nexport function run() { w.info(); }\n</script>\n<div/>\n";
    let result = resolve(&[("src/core.ts", CORE), ("src/View.svelte", svelte)]);
    let found: Vec<String> = calls_in(&result, "src/View.svelte")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(
        found,
        vec![CORE_INFO.to_string()],
        "embedded script behaviour"
    );
}

// -------------------------------------------------------------- hostile input

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    eprintln!("HOSTILE {label}: {:?}", start.elapsed());
    out
}

#[test]
fn hostile_100k_member_literal_with_many_member_calls_is_bounded() {
    let mut body = String::from("export const big = {\n");
    for i in 0..100_000 {
        body.push_str(&format!("  m{i}() {{}},\n"));
    }
    body.push_str("};\n");
    let mut user = String::from(
        "import { big } from './big';\nconst w = { ...big };\nexport function run() {\n",
    );
    for i in (0..100_000).step_by(10) {
        user.push_str(&format!("  w.m{i}();\n"));
    }
    user.push_str("}\n");
    let result = timed("100k-literal+10k-calls", || {
        resolve(&[("src/big.ts", &body), ("src/user.ts", &user)])
    });
    let count = calls_in(&result, "src/user.ts").len();
    eprintln!("HOSTILE 100k literal: {count} of 10000 member calls bound");
    assert!(count <= 10_000);
}

#[test]
fn hostile_10k_chained_aliases_with_calls_through_each_is_bounded() {
    let mut body = String::from("import { core } from './core';\nconst a0 = core;\n");
    for i in 1..10_000 {
        body.push_str(&format!("const a{i} = a{};\n", i - 1));
    }
    body.push_str("export function run() {\n");
    for i in (0..10_000).step_by(2) {
        body.push_str(&format!("  a{i}.info();\n"));
    }
    body.push_str("}\n");
    let start = Instant::now();
    let _ = timed("10k-aliases", || {
        resolve(&[("src/core.ts", CORE), ("src/user.ts", &body)])
    });
    assert!(start.elapsed().as_secs() < 10, "quadratic blow-up (base ab05b9e6 runs this corpus in 3.5s, this branch 18s, debug build): {:?}", start.elapsed());
}

#[test]
fn hostile_many_module_consts_with_calls_is_not_quadratic() {
    let mut body = String::new();
    for i in 0..30_000 {
        body.push_str(&format!("const c{i} = {{ f() {{}} }};\n"));
    }
    body.push_str("export function run() {\n");
    for i in 0..30_000 {
        body.push_str(&format!("  c{i}.f();\n"));
    }
    body.push_str("}\n");
    let start = Instant::now();
    let _ = timed("30k-consts+30k-calls", || {
        resolve(&[("src/user.ts", &body)])
    });
    assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
}

#[test]
fn hostile_5k_deep_nested_spreads_do_not_overflow_the_stack() {
    let depth = 5_000;
    let mut body = String::from("import { core } from './core';\nconst w = ");
    for _ in 0..depth {
        body.push_str("{ ...");
    }
    body.push_str("core");
    for _ in 0..depth {
        body.push_str(" }");
    }
    body.push_str(";\nexport function run() { w.info(); }\n");
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            timed("5k-nested-spread", || {
                let result = resolve(&[("src/core.ts", CORE), ("src/user.ts", &body)]);
                // Past MAX_NESTING the rest is unknown: never a wrong bind.
                calls_in(&result, "src/user.ts")
            })
        })
        .unwrap();
    let found = handle
        .join()
        .expect("stack overflow or panic on deep spreads");
    assert!(found.iter().all(|(_, t)| t == CORE_INFO));
}

#[test]
fn hostile_5mib_single_line_minified_file_finishes() {
    let mut body = String::from("import{core}from'./core';const w={...core};");
    let mut i = 0;
    while body.len() < 5 * 1024 * 1024 {
        body.push_str(&format!(
            "const k{i}={{...core,a{i}(){{}}}};Object.assign(k{i},core);"
        ));
        i += 1;
    }
    body.push_str("export function run(){w.info()}");
    let start = Instant::now();
    let _ = timed("5MiB-minified", || {
        resolve(&[("src/core.ts", CORE), ("src/user.ts", &body)])
    });
    assert!(start.elapsed().as_secs() < 60, "{:?}", start.elapsed());
}

// ---------------------------------------------------------------- determinism

fn read_ts_tree(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            read_ts_tree(&path, root, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ts" | "tsx" | "js")
        ) {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read_to_string(&path).unwrap()));
        }
    }
}

#[test]
fn the_ts_fixture_resolves_identically_on_one_thread_and_four() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy/ts");
    let mut files = Vec::new();
    read_ts_tree(&root, &root, &mut files);
    assert!(files.len() > 10, "fixture not found at {root:?}");
    let run = |threads: usize| -> Vec<String> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            use rayon::prelude::*;
            let exts: Vec<Extraction> = files.par_iter().map(|(p, s)| extract_file(p, s)).collect();
            let mut r = Resolver::new();
            r.index_extractions(&exts);
            let res = r.resolve_all(&exts).unwrap();
            res.edges
                .iter()
                .map(|e| {
                    format!(
                        "{}|{}|{}|{}|{:?}|{:?}",
                        e.source_file,
                        e.source_symbol,
                        e.target_file,
                        e.target_symbol,
                        e.edge_kind,
                        e.details
                    )
                })
                .collect()
        })
    };
    assert_eq!(run(1), run(4));
}
