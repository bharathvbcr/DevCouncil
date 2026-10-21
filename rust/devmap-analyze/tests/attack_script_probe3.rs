use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

fn dead(path: &str, src: &str) -> Vec<String> {
    reports(&[(path, src)])
        .iter()
        .filter(|x| !x.is_exempt)
        .map(|x| format!("{} conf={:?}", x.symbol_name, x.confidence))
        .collect()
}

const NS: &str = "namespace NS { export const a = 1; }\nconsole.log(NS.a);\n";
const NS2: &str = "namespace NS { export function f() { return 1; } }\nconsole.log(NS.f());\n";
const NS3: &str = "module Legacy { export const a = 1; }\nconsole.log(Legacy.a);\n";
const GLOBAL_AUG: &str =
    "namespace Util { export const x = 1; }\nfunction main() { return Util.x; }\nmain();\n";

// Same TS in a shebang script (branch now reports) vs a plain module.
#[test]
fn ts_namespace_in_script_is_live() {
    let d = dead("bin/t.ts", &format!("#!/usr/bin/env node\n{NS}"));
    assert!(d.is_empty(), "script: {d:?}");
}
#[test]
#[ignore = "ft-260158ce: a TS namespace const read as NS.a is reported dead at 0.9 in a plain module: pre-existing, independent of ScriptEntry"]
fn ts_namespace_in_plain_module_is_live() {
    let d = dead("src/t.ts", NS);
    assert!(d.is_empty(), "plain module (control): {d:?}");
}
#[test]
fn ts_namespace_fn_in_script_is_live() {
    let d = dead("bin/t.ts", &format!("#!/usr/bin/env node\n{NS2}"));
    assert!(d.is_empty(), "script: {d:?}");
}
#[test]
fn ts_module_kw_in_script_is_live() {
    let d = dead("bin/t.ts", &format!("#!/usr/bin/env node\n{NS3}"));
    assert!(d.is_empty(), "script: {d:?}");
}
#[test]
fn ts_namespace_used_in_main_is_live() {
    let d = dead("bin/t.ts", &format!("#!/usr/bin/env node\n{GLOBAL_AUG}"));
    assert!(d.is_empty(), "script: {d:?}");
}
// A non-shebang TS file with a top-level call has no ScriptEntry on either side; control.
#[test]
#[ignore = "ft-260158ce: same pre-existing TS namespace gap, no shebang"]
fn ts_script_without_shebang_control() {
    let d = dead("scripts/t.ts", NS);
    assert!(d.is_empty(), "no shebang control: {d:?}");
}
// Python: Num alias via string annotation in a plain module (control).
#[test]
#[ignore = "ft-f0af68af: a Python alias read only in a string annotation is reported dead in a plain module: pre-existing"]
fn py_string_annotation_plain_module_control() {
    let d = dead(
        "lib/s.py",
        "Num = int\ndef f(x: 'Num') -> 'Num':\n    return x\ndef g():\n    return f(1)\ndef h():\n    return g()\nh()\n",
    );
    assert!(d.is_empty(), "plain module: {d:?}");
}
#[test]
#[ignore = "ft-f0af68af: global X; X += 1 leaves X reported dead in a plain module: pre-existing"]
fn py_global_augassign_plain_module_control() {
    let d = dead(
        "lib/s.py",
        "COUNTER = 0\ndef inc():\n    global COUNTER\n    COUNTER += 1\ninc()\n",
    );
    assert!(d.is_empty(), "plain module: {d:?}");
}
