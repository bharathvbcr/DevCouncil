#[test]
fn probe() {
    for (label, path, src) in [
        (
            "ts_const",
            "a.ts",
            "export const VALUE = 42;\nexport const fn = () => {};\nconst local = 1;\n",
        ),
        (
            "ts_let_var",
            "b.ts",
            "export let mutable = 1;\nexport var legacy = 2;\n",
        ),
        (
            "js_const",
            "c.js",
            "export const CONFIG = { a: 1 };\nmodule.exports = { x: 1 };\n",
        ),
        ("py_const", "d.py", "CONST = 1\nclass C: pass\n"),
        (
            "go_const",
            "e.go",
            "package p\nconst Limit = 10\nvar Global = 1\n",
        ),
        (
            "rs_const",
            "f.rs",
            "pub const LIMIT: u32 = 10;\npub static GLOBAL: u32 = 1;\n",
        ),
    ] {
        let e = devmap_extract::extract_file(path, src);
        let syms: Vec<String> = e
            .symbols
            .iter()
            .map(|s| format!("{:?}:{}", s.kind, s.name))
            .collect();
        println!("{label}: {syms:?}");
    }
}
