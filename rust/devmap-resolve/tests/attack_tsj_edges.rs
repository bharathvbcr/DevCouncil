//! Attack round on the tsj lane (TS/JS barrels, CommonJS, literal constants,
//! Java records, `java.lang`), as it stands after the cut.
//!
//! The first block holds the breaks the attack found. Where the lane could not
//! prove a claim the claim was removed, so those tests now state what the lane
//! abstains from: nothing is bound, and the ledger says why. The bundler-alias
//! mapping left the lane altogether (a card carries it); its break and its
//! control stay, now as "unchanged from base". The second block holds controls,
//! shapes the attack tried that stay sound.

use std::fs;
use std::path::{Path, PathBuf};

use devmap_extract::collect_project_manifests;
use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::UnresolvedClass;
use devmap_resolve::{ResolutionResult, Resolver};

struct Tree(PathBuf);

impl Tree {
    fn new(files: &[(&str, &str)]) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "devmap-attack-tsj-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        for (relative, body) in files {
            let full = path.join(relative);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, body).unwrap();
        }
        Tree(path)
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

/// Resolve with no project manifests.
fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

/// Resolve with the manifests (tsconfig, bundler configs) the files on disk state.
fn resolve_with_manifests(files: &[(&str, &str)]) -> ResolutionResult {
    let tree = Tree::new(files);
    let manifests = collect_project_manifests(tree.root()).unwrap();
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_project_manifests(&manifests);
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

fn targets(result: &ResolutionResult, kind: EdgeKind, from: &str) -> Vec<String> {
    let mut found: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == kind && edge.source_symbol == from)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    found.sort();
    found.dedup();
    found
}

fn calls(result: &ResolutionResult, from: &str) -> Vec<String> {
    targets(result, EdgeKind::Calls, from)
}

/// The ledger reasons of the unresolved call rows `callee` has under `from`,
/// with the generic "no resolution ladder rung matched" text left out.
fn reasons(result: &ResolutionResult, from: &str, callee: &str) -> Vec<String> {
    result
        .unresolved
        .iter()
        .filter(|row| row.source_symbol == from && row.callee_name == callee)
        .filter_map(|row| match &row.resolution {
            devmap_resolve::model::Resolution::Unresolved { reason }
                if !reason.starts_with("no resolution ladder rung matched") =>
            {
                Some(reason.clone())
            }
            _ => None,
        })
        .collect()
}

/// The ledger reason a withheld CommonJS export gives.
const CJS_WITHHELD: &str = "commonjs export withheld: assigned more than once, or `module.exports` is reassigned in the file";

// ---------------------------------------------------------------------------
// Breaks
// ---------------------------------------------------------------------------

/// BREAK 1 (false narrowing, FIX-BEFORE-MERGE). `index_inert_values` records any
/// module-level `Variable` whose initializer is a literal, whatever its keyword.
/// `export let hook = null` is the standard callback slot: another statement
/// assigns a function to it and importers call it. Base binds the call
/// (`Calls`, ImportScoped); the lane demotes it to `References`.
#[test]
fn a_mutable_slot_that_starts_null_is_still_a_call_target() {
    let result = resolve(&[
        (
            "a.js",
            "export let hook = null;\n\
             export var legacy = 0;\n\
             export function setHook(h) { hook = h; }\n",
        ),
        (
            "b.js",
            "import { hook, legacy } from './a';\n\
             export function m() { hook(); legacy(); }\n",
        ),
        (
            "c.js",
            "const a = require('./a');\nexport function n() { a.hook(); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "b.js::m"),
        ["a.js::hook", "a.js::legacy"],
        "a `let`/`var` initialised to a literal is reassignable, so a call at it is a call"
    );
    assert_eq!(calls(&result, "c.js::n"), ["a.js::hook"]);
}

/// BREAK 2 (false narrowing, FIX-BEFORE-MERGE). `inert_values` is keyed by the
/// qualified name `file::name`. `exports.x = 3` pushes a literal-tagged `Variable`
/// `file::x`; a function `x` declared later in the file has the same qualified
/// name, so every call of the real function is demoted to `References`.
#[test]
fn a_function_sharing_a_name_with_a_literal_export_stays_callable() {
    let result = resolve(&[(
        "dup.js",
        "exports.x = 3;\nfunction x() {}\nfunction caller() { x(); }\n",
    )]);
    assert_eq!(calls(&result, "dup.js::caller"), ["dup.js::x"]);
}

/// BREAK 3 (false edge). `export *` never forwards `default` (ES 15.2.3.5), so a
/// default import of a star-only barrel, and `ns.main` over it, name nothing.
/// Base left the default import unresolved. The lane binds both to the source
/// file's default function. Cause (verified by the one-line patch described in
/// the report): `js_import_reexport_hops` records the hop
/// `d.ts::default -> (d.ts, main)` for `export default function main` because
/// `local != exported_name`, and `expand_glob_hops` publishes every hop key, so
/// the glob re-publishes `default`.
#[test]
fn a_default_import_of_a_star_only_barrel_binds_nothing() {
    let result = resolve(&[
        (
            "d.ts",
            "export default function main() {}\nexport function named() {}\n",
        ),
        ("dbarrel.ts", "export * from './d';\n"),
        (
            "use.ts",
            "import Foo from './dbarrel';\nexport function go() { Foo(); }\n",
        ),
    ]);
    assert!(
        calls(&result, "use.ts::go").is_empty(),
        "`export *` never forwards `default`: {:?}",
        calls(&result, "use.ts::go")
    );
    assert_eq!(
        reasons(&result, "use.ts::go", "Foo"),
        ["default is not re-exported by `export *`"],
        "the abstention names its reason"
    );
}

/// FOLLOW-UP (pre-existing). `expand_glob_hops` republishes every symbol of the
/// source file (`file_symbols`), exported or not and including a default
/// export's local name. On base the named-import form already binds
/// (`import { main, priv } from './dbarrel'` gives `Calls` to both, verified
/// with `devmap-main` on this fixture); the lane's namespace-over-a-barrel path
/// (`ns.main()`) newly inherits it. Both name things the module does not export.
#[test]
#[ignore = "pre-existing on dbd0193a; card ft-e3a3bfa2"]
fn a_star_barrel_publishes_only_what_its_source_exports() {
    let result = resolve(&[
        (
            "d.ts",
            "export default function main() {}\nexport function named() {}\nfunction priv() {}\n",
        ),
        ("dbarrel.ts", "export * from './d';\n"),
        (
            "use.ts",
            "import * as ns from './dbarrel';\n\
             import { main, priv } from './dbarrel';\n\
             export function go() { ns.main(); ns.priv(); main(); priv(); ns.named(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "use.ts::go"), ["d.ts::named"]);
}

/// BREAK 4. The last top-level assignment to `exports.x` is the value a
/// `require` sees, and the lane kept the first. A name assigned more than once
/// is not a provable export, so the lane withholds it: nothing is bound (base
/// bound nothing either) and the row says why.
#[test]
fn a_commonjs_export_assigned_twice_is_withheld() {
    let result = resolve(&[
        (
            "twice.js",
            "exports.dupname = function one() {};\nexports.dupname = function two() {};\n",
        ),
        (
            "use.js",
            "const t = require('./twice');\nfunction u() { t.dupname(); }\n",
        ),
    ]);
    assert!(
        calls(&result, "use.js::u").is_empty(),
        "{:?}",
        calls(&result, "use.js::u")
    );
    assert_eq!(
        reasons(&result, "use.js::u", "dupname"),
        [CJS_WITHHELD],
        "the abstention names its reason"
    );
}

/// BREAK 5. After `module.exports = run`, the local `exports` still names the
/// old object, so `exports.late = ...` is not published (the classic CommonJS
/// trap). A file that reassigns `module.exports` publishes no `exports.x`: nothing
/// is bound and the row says why.
#[test]
fn exports_assigned_after_module_exports_is_replaced_is_withheld() {
    let result = resolve(&[
        (
            "mixed.js",
            "function run() {}\nmodule.exports = run;\nexports.late = function late() {};\n",
        ),
        (
            "use.js",
            "const { late } = require('./mixed');\nfunction u() { late(); }\n",
        ),
    ]);
    assert!(
        calls(&result, "use.js::u").is_empty(),
        "{:?}",
        calls(&result, "use.js::u")
    );
    assert_eq!(reasons(&result, "use.js::u", "late"), [CJS_WITHHELD]);
}

/// BREAK 6, removed with the bundler-alias mapping. A bundler config has no
/// directory scope, so `apps/web/vite.config.ts` once mapped `@` for every file
/// in the repository and `apps/admin`'s `@/lib/util` bound to `apps/web`'s file.
/// The lane no longer reads bundler configs: the import is what base made it, an
/// outside module, and nothing is bound.
#[test]
fn a_bundler_alias_binds_nothing_in_any_package() {
    let result = resolve_with_manifests(&[
        (
            "apps/web/vite.config.ts",
            "import path from 'path';\n\
             export default { resolve: { alias: { '@': path.resolve(__dirname, 'src') } } };\n",
        ),
        ("apps/web/src/lib/util.ts", "export function util() {}\n"),
        ("apps/admin/src/lib/util.ts", "export function util() {}\n"),
        (
            "apps/admin/src/main.ts",
            "import { util } from '@/lib/util';\nexport function adminUse() { util(); }\n",
        ),
        (
            "apps/web/src/main.ts",
            "import { util } from '@/lib/util';\nexport function webUse() { util(); }\n",
        ),
    ]);
    assert!(calls(&result, "apps/admin/src/main.ts::adminUse").is_empty());
    assert!(calls(&result, "apps/web/src/main.ts::webUse").is_empty());
}

/// BREAK 7 (lost true edge vs base; the ambiguity itself is
/// pre-existing for a plain class, see the control below). Base bound
/// `new Canon(1, 2)` to the explicit canonical constructor with a `Calls` edge
/// (the record was no symbol, so the constructor was the only `Canon`). With the
/// record indexed, `Canon` names the record and `Canon.Canon` the constructor,
/// the call is `AmbiguousGlobal`, and the `Calls` edge is gone.
#[test]
fn a_record_with_an_explicit_canonical_constructor_keeps_its_new_call() {
    let result = resolve(&[
        (
            "a/Canon.java",
            "package a;\npublic record Canon(int lo, int hi) {\n  public Canon(int lo, int hi) { this.lo = lo; this.hi = hi; }\n}\n",
        ),
        (
            "b/U.java",
            "package b;\nimport a.*;\npublic class U {\n  void go() { new Canon(1, 2); }\n}\n",
        ),
    ]);
    let got = calls(&result, "b/U.java::U.go");
    assert!(
        got.iter()
            .any(|target| target.starts_with("a/Canon.java::Canon")),
        "no Calls edge for `new Canon(..)`: {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Controls: attacks that stay sound
// ---------------------------------------------------------------------------

#[test]
fn control_a_star_collision_binds_neither_source() {
    let result = resolve(&[
        (
            "a.ts",
            "export function dup() {}\nexport function onlyA() {}\n",
        ),
        (
            "b.ts",
            "export function dup() {}\nexport function onlyB() {}\n",
        ),
        ("barrel.ts", "export * from './a';\nexport * from './b';\n"),
        (
            "use.ts",
            "import { dup, onlyA, onlyB } from './barrel';\n\
             import * as bb from './barrel';\n\
             export function go() { dup(); onlyA(); onlyB(); bb.dup(); bb.onlyB(); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "use.ts::go"),
        ["a.ts::onlyA", "b.ts::onlyB"],
        "an ambiguous star name is excluded (ES 15.2.3.5)"
    );
}

#[test]
fn control_a_local_declaration_and_an_explicit_reexport_outrank_a_star() {
    let result = resolve(&[
        (
            "a.ts",
            "export function dup() {}\nexport function onlyA() {}\n",
        ),
        ("b.ts", "export function dup() {}\n"),
        (
            "shadow.ts",
            "export * from './a';\nexport function dup() {}\n",
        ),
        (
            "expl.ts",
            "export { dup } from './b';\nexport * from './a';\n",
        ),
        (
            "use.ts",
            "import * as s from './shadow';\nimport * as e from './expl';\n\
             export function go() { s.dup(); s.onlyA(); e.dup(); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "use.ts::go"),
        ["a.ts::onlyA", "b.ts::dup", "shadow.ts::dup"]
    );
}

#[test]
fn control_circular_barrels_resolve_each_member_once() {
    let result = resolve(&[
        (
            "c1.ts",
            "export * from './c2';\nexport function c1fn() {}\n",
        ),
        (
            "c2.ts",
            "export * from './c1';\nexport function c2fn() {}\n",
        ),
        (
            "use.ts",
            "import * as cc from './c2';\nexport function go() { cc.c1fn(); cc.c2fn(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "use.ts::go"), ["c1.ts::c1fn", "c2.ts::c2fn"]);
}

#[test]
fn control_a_function_valued_const_is_still_a_call_target() {
    let result = resolve(&[
        (
            "a.ts",
            "export const f = () => 1;\nexport const LIMIT = 3;\nexport const alias = f;\n",
        ),
        (
            "b.ts",
            "import { f, LIMIT, alias } from './a';\n\
             export function m() { f(); LIMIT(); alias(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "b.ts::m"), ["a.ts::alias", "a.ts::f"]);
}

#[test]
fn control_commonjs_inside_a_function_or_behind_a_parameter_is_not_published() {
    let result = resolve(&[
        (
            "shadow.js",
            "function sh(exports) { exports.p = function p() {}; }\n\
             function inner() { exports.q = function q() {}; }\n",
        ),
        (
            "use.js",
            "const s = require('./shadow');\nfunction u() { s.p(); s.q(); }\n",
        ),
    ]);
    assert!(calls(&result, "use.js::u").is_empty());
}

#[test]
fn control_an_alias_onto_an_unindexed_directory_is_inert() {
    let result = resolve_with_manifests(&[
        (
            "vite.config.ts",
            "import path from 'path';\n\
             export default { resolve: { alias: {\n\
               'react': path.resolve(__dirname, 'node_modules/react'),\n\
               'built': path.resolve(__dirname, 'dist'),\n\
               'out': path.resolve(__dirname, '../outside'),\n\
             } } };\n",
        ),
        (
            "src/m.ts",
            "import { useState } from 'react';\nexport function go() { useState(); }\n",
        ),
    ]);
    let class = result
        .unresolved
        .iter()
        .find(|row| row.callee_name == "useState")
        .map(|row| row.class.clone());
    assert!(
        matches!(class, Some(UnresolvedClass::External { .. })),
        "{class:?}"
    );
}

#[test]
fn control_a_generic_file_never_claims_java_lang() {
    let result = resolve(&[
        (
            "p/Box.java",
            "package p;\npublic class Box<Record> { Record held; Number n; }\n",
        ),
        (
            "p/Plain.java",
            "package p;\npublic class Plain { Record r; Number n; String s; }\n",
        ),
    ]);
    let class_of = |file: &str, name: &str| {
        result
            .unresolved
            .iter()
            .find(|row| row.source_file == file && row.callee_name == name)
            .map(|row| row.class.clone())
    };
    assert!(!matches!(
        class_of("p/Box.java", "Record"),
        Some(UnresolvedClass::External { .. })
    ));
    assert!(!matches!(
        class_of("p/Box.java", "Number"),
        Some(UnresolvedClass::External { .. })
    ));
    assert!(matches!(
        class_of("p/Plain.java", "Record"),
        Some(UnresolvedClass::External { .. })
    ));
}

#[test]
fn control_a_user_class_named_like_a_java_lang_type_wins() {
    let result = resolve(&[
        (
            "p/Process.java",
            "package p;\npublic class Process { public void run() {} }\n",
        ),
        (
            "p/User.java",
            "package p;\npublic class User { void f(Process x) { x.run(); } }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "p/User.java::User.f"),
        ["p/Process.java::Process.run"]
    );
}

#[test]
fn control_a_single_type_import_keeps_its_own_module_not_java_lang() {
    let result = resolve(&[(
        "p/A.java",
        "package p;\nimport com.foo.Process;\npublic class A { void f(Process x, Record r) { } }\n",
    )]);
    let module_of = |name: &str| {
        result
            .unresolved
            .iter()
            .find_map(|row| match (&row.class, row.callee_name == name) {
                (UnresolvedClass::External { module }, true) => Some(module.clone()),
                _ => None,
            })
    };
    assert_eq!(module_of("Process").as_deref(), Some("com.foo.Process"));
    assert_eq!(module_of("Record").as_deref(), Some("java.lang.Record"));
}
