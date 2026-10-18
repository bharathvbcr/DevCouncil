//! Attack probe (not a regression test): dumps the extraction of every
//! non-Swift accuracy fixture plus synthetic export shapes to `dump.txt` at
//! the worktree root, so two builds can be byte-compared.
//! Run: cargo test -p devmap-extract --test non_swift_byte_compare -- --ignored

use devmap_extract::extract_file;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

const SYNTHETIC: &[(&str, &str)] = &[
    ("dup.ts", "export function a() {}\nexport function a() {}\nexport const A = 1;\nexport { a as b };\nexport { A as a };\nexport default function () {}\nexport default class Foo {}\nexport { Foo as default };\nexport * from './x';\nexport { q } from './y';\nexport { q as a2 } from './y';\nfunction q() {}\nexport { q };\n"),
    ("case.ts", "export const Foo = 1;\nexport const foo = 2;\nexport { Foo as foo };\nexport class default_ {}\n"),
    ("alias.ts", "const local = 1;\nexport { local as Public };\nexport function local2() {}\nexport { local2 as local };\nexport { local };\n"),
    ("dflt.ts", "export default function main() {}\nexport function main() {}\nexport { main as default };\nexport default main;\n"),
    ("dflt2.tsx", "export default function Comp() { return <div/>; }\nexport const Comp2 = () => <b/>;\nexport { Comp2 as default };\n"),
    ("m.js", "function f(){}\nmodule.exports = { f };\nexports.g = f;\nexport { f as h, f as i };\nexport { f };\n"),
    ("p.py", "__all__ = ['a','a','b']\ndef a(): pass\ndef b(): pass\nclass A: pass\ndef a(): pass\n"),
    ("g.go", "package p\nfunc A() {}\nfunc a() {}\ntype A2 struct{}\nvar Same = 1\nconst Same2 = 2\n"),
    ("k.kt", "class A\nclass a\nfun A() {}\nobject A\npublic val x = 1\nprivate val y = 2\n"),
    ("r.rs", "pub fn a() {}\npub fn A() {}\npub use self::m::a as b;\npub use self::m::a;\nmod m { pub fn a() {} }\npub struct S;\npub struct S;\n"),
    ("c.cs", "public class A { public void A() {} }\npublic class a {}\n"),
    ("j.java", "public class A { public void A() {} public int A; }\nclass a {}\n"),
    ("rb.rb", "class A; def a; end; def a; end; end\nmodule A; end\n"),
    ("p.php", "<?php\nclass A {}\nclass a {}\nfunction A() {}\n"),
];

#[test]
#[ignore]
fn dump() {
    let fixtures = Path::new("/Users/bharath/Code/devtools/DevCouncil/.claude/worktrees/agent-a345ecfa01fea3074/rust/testdata/accuracy");
    let mut files = Vec::new();
    walk(fixtures, &mut files);
    files.sort();
    let mut out = String::new();
    let mut n = 0;
    for path in files {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "go" | "kt" | "kts" | "py" | "rs" | "ts" | "tsx" | "js") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(fixtures)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let ex = extract_file(&rel, &source);
        writeln!(out, "=== {rel}\n{ex:#?}").unwrap();
        n += 1;
    }
    for (name, src) in SYNTHETIC {
        let ex = extract_file(name, src);
        writeln!(out, "=== synthetic {name}\n{ex:#?}").unwrap();
        n += 1;
    }
    let dest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dump.txt");
    std::fs::write(dest, out).unwrap();
    eprintln!("dumped {n} files");
}
