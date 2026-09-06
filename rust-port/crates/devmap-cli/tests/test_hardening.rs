use devmap_analyze::*;
use devmap_extract::*;
use devmap_query::*;
use devmap_resolve::*;
use devmap_store::*;
use std::time::Instant;

#[test]
fn test_treesitter_extraction() {
    // Test Python
    let py_code = r#"
import os
from sys import path

def my_func(a, b):
    print(a)

class MyClass:
    def method(self):
        my_func(1, 2)
"#;
    let py_ext = extract_file("src/test.py", py_code);
    assert_eq!(py_ext.language, "python");
    assert!(py_ext
        .symbols
        .iter()
        .any(|s| s.name == "my_func" && s.kind == SymbolKind::Function));
    assert!(py_ext
        .symbols
        .iter()
        .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
    // The frozen Python baseline identifies a method as `<file>::<Type>.<name>`
    // (see testdata/golden/python_app/nodes.json -> `app.py::MyClass.execute`),
    // and `codeintel` recovers the owning path by splitting on the FIRST `::`.
    // Emitting a second `::` here would fork symbol identity from that baseline.
    assert!(py_ext.symbols.iter().any(|s| {
        s.name == "method"
            && s.kind == SymbolKind::Method
            && s.qualified_name == "src/test.py::MyClass.method"
            && s.parent_symbol.as_deref() == Some("src/test.py::MyClass")
    }));
    assert!(py_ext.imports.iter().any(|i| i.module_specifier == "os"));
    assert!(py_ext
        .imports
        .iter()
        .any(|i| i.module_specifier == "sys" && i.imported_names.contains(&"path".to_string())));
    assert!(py_ext.calls.iter().any(|c| c.callee_name == "my_func"));

    // Test JS/TS
    let ts_code = r#"
import { A, B as C } from './module_specifier';

function doSomething() {}

const arrowFn = () => {
    doSomething();
};

export class Service {}
"#;
    let ts_ext = extract_file("src/test.ts", ts_code);
    assert_eq!(ts_ext.language, "typescript");
    assert!(ts_ext
        .symbols
        .iter()
        .any(|s| s.name == "doSomething" && s.kind == SymbolKind::Function));
    assert!(ts_ext
        .symbols
        .iter()
        .any(|s| s.name == "Service" && s.kind == SymbolKind::Class));
    assert!(ts_ext
        .imports
        .iter()
        .any(|i| i.module_specifier == "./module_specifier"
            && i.imported_names.contains(&"A".to_string())
            && i.imported_names.contains(&"B".to_string())));

    // Test Rust
    let rs_code = r#"
use std::collections::HashMap;

pub struct Data {
    id: i32,
}

pub enum State {
    Active,
}

impl Data {
    pub fn new() -> Self {
        Data { id: 0 }
    }
}

fn helper() {}
"#;
    let rs_ext = extract_file("src/test.rs", rs_code);
    assert_eq!(rs_ext.language, "rust");
    assert!(rs_ext
        .symbols
        .iter()
        .any(|s| s.name == "Data" && s.kind == SymbolKind::Struct));
    assert!(rs_ext
        .symbols
        .iter()
        .any(|s| s.name == "State" && s.kind == SymbolKind::Enum));
    // `fn new` lives in `impl Data`, so it is Data's method — not a second
    // top-level `new`. One owner emits it, qualified by its impl type.
    assert!(rs_ext.symbols.iter().any(|s| {
        s.name == "new"
            && s.kind == SymbolKind::Method
            && s.qualified_name == "src/test.rs::Data.new"
            && s.parent_symbol.as_deref() == Some("src/test.rs::Data")
    }));
    assert_eq!(
        rs_ext.symbols.iter().filter(|s| s.name == "new").count(),
        1,
        "impl methods must be emitted exactly once"
    );
    assert!(rs_ext
        .symbols
        .iter()
        .any(|s| s.name == "helper" && s.kind == SymbolKind::Function));
    assert!(rs_ext
        .imports
        .iter()
        .any(|i| i.module_specifier == "std::collections::HashMap"));

    // Test Go
    let go_code = r#"
package main

import "fmt"

type Config struct {
    Port int
}

func Process() {
    fmt.Println("test")
}
"#;
    let go_ext = extract_file("src/test.go", go_code);
    assert_eq!(go_ext.language, "go");
    assert!(go_ext.symbols.iter().any(|s| s.name == "Config"));
    assert!(go_ext
        .symbols
        .iter()
        .any(|s| s.name == "Process" && s.kind == SymbolKind::Function));
    assert!(go_ext.imports.iter().any(|i| i.module_specifier == "fmt"));

    // Test malformed code. Tree-sitter's error tree is authoritative; a regex
    // fallback must not fabricate a valid symbol from invalid syntax (X6/X7).
    let bad_code = r#"
def fn(((:
    print(
"#;
    let bad_ext = extract_file("src/bad.py", bad_code);
    assert_eq!(bad_ext.language, "python");
    assert!(matches!(
        bad_ext.parse_outcome,
        ParseOutcome::Partial { ref error_ranges } if !error_ranges.is_empty()
    ));
    assert!(!bad_ext.symbols.iter().any(|s| s.name == "fn"));
}

#[test]
fn test_export_detection() {
    // Python
    let py_code = "def my_func(): pass\nclass MyClass: pass\n";
    let py_ext = extract_file("src/test.py", py_code);
    for sym in &py_ext.symbols {
        if sym.kind != SymbolKind::File {
            assert!(
                !sym.is_exported,
                "Python symbols should not be marked exported by default"
            );
        }
    }

    // JS/TS
    let ts_code = "export function a() {}\nfunction b() {}\nexport class C {}\nclass D {}\n";
    let ts_ext = extract_file("src/test.ts", ts_code);
    assert!(
        ts_ext
            .symbols
            .iter()
            .find(|s| s.name == "a")
            .unwrap()
            .is_exported
    );
    assert!(
        !ts_ext
            .symbols
            .iter()
            .find(|s| s.name == "b")
            .unwrap()
            .is_exported
    );
    assert!(
        ts_ext
            .symbols
            .iter()
            .find(|s| s.name == "C")
            .unwrap()
            .is_exported
    );
    assert!(
        !ts_ext
            .symbols
            .iter()
            .find(|s| s.name == "D")
            .unwrap()
            .is_exported
    );

    // Rust
    let rs_code = "pub fn a() {}\nfn b() {}\npub struct C {}\nstruct D {}\n";
    let rs_ext = extract_file("src/test.rs", rs_code);
    assert!(
        rs_ext
            .symbols
            .iter()
            .find(|s| s.name == "a")
            .unwrap()
            .is_exported
    );
    assert!(
        !rs_ext
            .symbols
            .iter()
            .find(|s| s.name == "b")
            .unwrap()
            .is_exported
    );
    assert!(
        rs_ext
            .symbols
            .iter()
            .find(|s| s.name == "C")
            .unwrap()
            .is_exported
    );
    assert!(
        !rs_ext
            .symbols
            .iter()
            .find(|s| s.name == "D")
            .unwrap()
            .is_exported
    );

    // Go
    let go_code = "package main\nfunc Exported() {}\nfunc unexported() {}\ntype ExportedType struct {}\ntype unexportedType struct {}\n";
    let go_ext = extract_file("src/test.go", go_code);
    assert!(
        go_ext
            .symbols
            .iter()
            .find(|s| s.name == "Exported")
            .unwrap()
            .is_exported
    );
    assert!(
        !go_ext
            .symbols
            .iter()
            .find(|s| s.name == "unexported")
            .unwrap()
            .is_exported
    );
    if let Some(t) = go_ext.symbols.iter().find(|s| s.name == "ExportedType") {
        assert!(t.is_exported);
    }
}

#[test]
fn test_named_import_binding() {
    let py_ext = extract_file(
        "src/test.py",
        "from foo import bar, baz\nfrom qux import a as b",
    );
    let i1 = py_ext
        .imports
        .iter()
        .find(|i| i.module_specifier == "foo")
        .unwrap();
    assert!(i1.imported_names.contains(&"bar".to_string()));
    assert!(i1.imported_names.contains(&"baz".to_string()));

    let i2 = py_ext
        .imports
        .iter()
        .find(|i| i.module_specifier == "qux")
        .unwrap();
    assert!(i2.imported_names.contains(&"a".to_string()));
    // Note: Alias extraction logic depends on the specific treesitter implementation for alias.
    // Assuming the test instructions meant just testing `imported_names` captures the original name correctly.

    let ts_ext = extract_file("src/test.ts", "import { X, Y as Z } from './mod'");
    let i3 = ts_ext
        .imports
        .iter()
        .find(|i| i.module_specifier == "./mod")
        .unwrap();
    assert!(i3.imported_names.contains(&"X".to_string()));
    assert!(i3.imported_names.contains(&"Y".to_string()));
}

#[test]
fn test_import_resolution() {
    // JS relative
    let ts_ext1 = extract_file("src/parent/child/mod.ts", "export function target() {}");
    let ts_ext2 = extract_file(
        "src/parent/child/sibling.ts",
        "import { target } from './mod'",
    );
    let ts_ext3 = extract_file(
        "src/parent/cousin.ts",
        "import { target } from './child/mod'",
    );
    let ts_ext4 = extract_file(
        "src/parent/child/deep/nested.ts",
        "import { target } from '../../child/mod'",
    ); // ../../child -> parent/child

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[
        ts_ext1.clone(),
        ts_ext2.clone(),
        ts_ext3.clone(),
        ts_ext4.clone(),
    ]);
    let res = resolver.resolve_all(&[ts_ext1, ts_ext2, ts_ext3, ts_ext4]);

    // sibling.ts -> mod.ts
    assert!(res
        .edges
        .iter()
        .any(|e| e.source_file == "src/parent/child/sibling.ts"
            && e.target_file == "src/parent/child/mod.ts"));

    // cousin.ts -> mod.ts
    assert!(res
        .edges
        .iter()
        .any(|e| e.source_file == "src/parent/cousin.ts"
            && e.target_file == "src/parent/child/mod.ts"));

    // Python relative
    let py_ext1 = extract_file("src/pkg/mod.py", "def x(): pass");
    let py_ext2 = extract_file("src/pkg/other.py", "from .mod import x");

    let mut py_resolver = Resolver::new();
    py_resolver.index_extractions(&[py_ext1.clone(), py_ext2.clone()]);
    let py_res = py_resolver.resolve_all(&[py_ext1, py_ext2]);
    assert!(py_res
        .edges
        .iter()
        .any(|e| e.source_file == "src/pkg/other.py" && e.target_file == "src/pkg/mod.py"));

    // Rust crate::
    let rs_ext1 = extract_file("src/module.rs", "pub fn x() {}");
    let rs_ext2 = extract_file("src/main.rs", "use crate::module::x;");
    let mut rs_resolver = Resolver::new();
    rs_resolver.index_extractions(&[rs_ext1.clone(), rs_ext2.clone()]);
    let rs_res = rs_resolver.resolve_all(&[rs_ext1, rs_ext2]);
    assert!(rs_res
        .edges
        .iter()
        .any(|e| e.source_file == "src/main.rs" && e.target_file == "src/module.rs"));
}

#[test]
fn test_wiring_exemptions() {
    let py_decorator = extract_file("src/app.py", "@app.route('/test')\ndef handle(): pass");
    assert!(py_decorator
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::FrameworkDecorator));

    let py_init = extract_file("src/__init__.py", "from . import a\nfrom .b import c\n");
    assert!(py_init
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::ReExportPackage));

    let dockerfile = extract_file("Dockerfile", "FROM ubuntu:latest\nCMD [\"echo\"]");
    assert!(dockerfile
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::Launcher));

    let cargo = extract_file(
        "Cargo.toml",
        "[package]\nname=\"a\"\n[[bin]]\nname=\"b\"\npath=\"src/main.rs\"",
    );
    assert!(cargo
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::ScriptEntry));

    let pyproject = extract_file(
        "pyproject.toml",
        "[project.scripts]\nmy-script = \"my_pkg.mod:func\"",
    );
    assert!(pyproject
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::ScriptEntry));

    let pkg_json = extract_file("package.json", "{\"bin\": {\"cmd\": \"./bin/cmd.js\"}}");
    assert!(pkg_json
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::ScriptEntry));

    let rs_main = extract_file("src/main.rs", "fn main() {}");
    assert!(rs_main
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::ScriptEntry));

    let go_main = extract_file("src/main.go", "package main\nfunc main() {}");
    assert!(go_main
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::ScriptEntry));

    // Symbol-scoped annotations target a `qualified_name`, never the file
    // path. A file-scoped target would exempt every symbol in the file.
    let rs_test = extract_file(
        "src/engine.rs",
        "fn compute() -> u32 { 1 }\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn compute_works() { assert_eq!(compute(), 1); }\n}\nfn orphan() {}\n",
    );
    let harness = rs_test
        .wiring
        .iter()
        .find(|w| w.kind == WiringKind::RuntimeEntryPoint)
        .expect("#[test] must produce a runtime entry point annotation");
    assert_eq!(harness.target_symbol, "src/engine.rs::compute_works");
    assert!(
        rs_test
            .wiring
            .iter()
            .all(|w| w.target_symbol != "src/engine.rs"),
        "a #[test] fn must not raise any file-scoped annotation: {:?}",
        rs_test.wiring
    );

    let rs_trait = extract_file(
        "src/handler.rs",
        "pub trait Handler { fn handle(&self); }\npub struct Real;\nimpl Handler for Real { fn handle(&self) {} }\n",
    );
    // BOTH the trait's declaration and the implementation need the exemption,
    // and they are now distinct symbols (SC11/SC6b): the declaration is
    // `Handler.handle`, the implementation is `Real.handle`. Asserting only one
    // would let the other silently become confidently dead — which is exactly
    // what happened when impl methods were requalified and this assertion kept
    // passing against the declaration alone.
    for target in [
        "src/handler.rs::Handler.handle",
        "src/handler.rs::Real.handle",
    ] {
        assert!(
            rs_trait
                .wiring
                .iter()
                .any(|w| w.kind == WiringKind::StructuralExempt && w.target_symbol == target),
            "{target} cannot carry `pub`, so it needs a structural annotation: {:?}",
            rs_trait.wiring
        );
    }
    // The declaration and the implementation must not collapse into one
    // identity; a shared qualified name is a broken graph join key.
    assert_ne!(
        rs_trait
            .symbols
            .iter()
            .filter(|s| s.name == "handle")
            .count(),
        1,
        "the trait's declared method and its implementation must both be symbols: {:?}",
        rs_trait
            .symbols
            .iter()
            .map(|s| &s.qualified_name)
            .collect::<Vec<_>>()
    );

    let go_init = extract_file("svc/registry.go", "package svc\nfunc init() {}\n");
    assert!(
        go_init
            .wiring
            .iter()
            .any(|w| w.kind == WiringKind::RuntimeEntryPoint
                && w.target_symbol == "svc/registry.go::init"),
        "func init() is unreferenceable and must be annotated: {:?}",
        go_init.wiring
    );

    let go_iface = extract_file(
        "svc/worker.go",
        "package svc\ntype runner interface { run() error }\ntype job struct{}\nfunc (j *job) run() error { return nil }\n",
    );
    assert!(
        go_iface
            .wiring
            .iter()
            .any(|w| w.kind == WiringKind::StructuralExempt
                && w.target_symbol == "svc/worker.go::job.run"),
        "a same-file interface implementation must be annotated: {:?}",
        go_iface.wiring
    );

    let tsx_hook = extract_file(
        "web/Widget.tsx",
        "class Widget extends HTMLElement { connectedCallback() {} }\n",
    );
    assert!(
        tsx_hook
            .wiring
            .iter()
            .any(|w| w.kind == WiringKind::RuntimeEntryPoint
                && w.target_symbol == "web/Widget.tsx::Widget.connectedCallback"),
        "a custom-element lifecycle hook must be annotated: {:?}",
        tsx_hook.wiring
    );

    let py_hook = extract_file("src/helpers.py", "def pytest_configure(cfg): pass\n");
    assert!(
        py_hook
            .wiring
            .iter()
            .any(|w| w.kind == WiringKind::RuntimeEntryPoint
                && w.target_symbol == "src/helpers.py::pytest_configure"),
        "a pytest plugin hook must be annotated: {:?}",
        py_hook.wiring
    );
}

/// A class method is externally reachable exactly when its class is exported.
///
/// `method_definition.parent()` is `class_body`, never `export_statement`, so
/// the previous parent check could not return true for any method of any
/// class — every method of every exported class read as unexported.
#[test]
fn test_class_methods_inherit_the_export_status_of_their_class() {
    let ext = extract_file(
        "web/App.tsx",
        "export default class App extends React.Component {\n  render() { return null; }\n}\nclass Local { helper() {} }\n",
    );
    let method = |name: &str| {
        ext.symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("{name} should be extracted"))
    };
    assert!(
        method("render").is_exported,
        "a method of an exported class is reachable from outside the module"
    );
    assert!(
        !method("helper").is_exported,
        "a method of a module-local class must stay unexported"
    );
}

/// Runtime, framework, and harness entry points, one case per class of defect.
///
/// Each case pairs the entry point with an ordinary neighbour in the same file.
/// The neighbour is the control: it proves the new exemption is symbol-scoped
/// and did not widen into a file-level blanket, which is exactly how the
/// existing `FrameworkDecorator` rule over-exempts today.
#[test]
fn test_runtime_entry_points_are_exempt_without_exempting_their_file() {
    struct Case {
        path: &'static str,
        source: &'static str,
        exempt: &'static [&'static str],
        still_dead: &'static str,
    }

    let cases = [
        Case {
            path: "src/handler.rs",
            source: "pub trait Handler { fn handle(&self); }\npub struct Real;\nimpl Handler for Real { fn handle(&self) {} }\nimpl Real { fn helper(&self) {} }\n",
            exempt: &["Handler.handle"],
            still_dead: "Real.helper",
        },
        Case {
            path: "src/engine.rs",
            source: "#[cfg(test)]\nmod tests {\n    #[test]\n    fn compute_works() {}\n    #[tokio::test]\n    async fn async_works() {}\n    #[bench]\n    fn benched() {}\n}\nfn orphan() {}\n",
            exempt: &["compute_works", "async_works", "benched"],
            still_dead: "orphan",
        },
        Case {
            path: "src/ffi.rs",
            source: "#[no_mangle]\nextern \"C\" fn ffi_entry() {}\n#[ctor]\nfn boot() {}\n#[proc_macro_derive(Thing)]\nfn derive_thing() {}\nfn ffi_helper() {}\n",
            exempt: &["ffi_entry", "boot", "derive_thing"],
            still_dead: "ffi_helper",
        },
        Case {
            path: "src/bin/tool.rs",
            source: "fn main() {}\nfn tool_helper() {}\n",
            exempt: &["main"],
            still_dead: "tool_helper",
        },
        Case {
            path: "svc/registry.go",
            source: "package svc\nfunc init() {}\nfunc unusedHelper() {}\n",
            exempt: &["init"],
            still_dead: "unusedHelper",
        },
        Case {
            path: "cmd/app/run.go",
            source: "package main\nfunc main() {}\nfunc runHelper() {}\n",
            exempt: &["main"],
            still_dead: "runHelper",
        },
        Case {
            path: "svc/worker.go",
            source: "package svc\ntype runner interface { run() error }\ntype job struct{}\nfunc (j *job) run() error { return nil }\nfunc (j *job) unusedMethod() {}\n",
            exempt: &["job.run"],
            still_dead: "job.unusedMethod",
        },
        Case {
            path: "src/helpers.py",
            source: "def test_thing(): pass\ndef pytest_configure(cfg): pass\ndef setup_module(mod): pass\ndef ordinary(): pass\n",
            exempt: &["test_thing", "pytest_configure", "setup_module"],
            still_dead: "ordinary",
        },
        Case {
            path: "web/Widget.tsx",
            source: "class Widget extends HTMLElement {\n  connectedCallback() {}\n  unusedMethod() {}\n}\n",
            exempt: &["Widget.connectedCallback"],
            still_dead: "Widget.unusedMethod",
        },
    ];

    for case in &cases {
        let ext = extract_file(case.path, case.source);
        let mut resolver = Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&ext));
        let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
        let analysis = analyze(std::slice::from_ref(&ext), &resolution);

        let confidently_dead = |name: &str| {
            analysis.dead_symbols.iter().any(|report| {
                report.symbol_name == name && !report.is_exempt && report.confidence >= 0.9
            })
        };

        for name in case.exempt {
            assert!(
                !confidently_dead(name),
                "{}::{name} is a runtime entry point and must not be confidently dead: {:?}",
                case.path,
                analysis.dead_symbols
            );
            let report = analysis
                .dead_symbols
                .iter()
                .find(|report| report.symbol_name == *name);
            if let Some(report) = report {
                assert!(
                    report
                        .exemption_reason
                        .as_deref()
                        .is_some_and(|reason| !reason.is_empty()),
                    "{}::{name} must carry a concrete exemption reason, got {:?}",
                    case.path,
                    report.exemption_reason
                );
            }
        }

        assert!(
            confidently_dead(case.still_dead),
            "{}::{} is an ordinary unused symbol and must stay confidently dead — \
             the entry-point exemption leaked to file scope: {:?}",
            case.path,
            case.still_dead,
            analysis.dead_symbols
        );
    }
}

#[test]
fn test_dead_code_detection() {
    let f1 = extract_file(
        "src/f1.py",
        "def dead(): pass\ndef alive(): pass\ndef _exempt(): pass\n",
    );
    let f2 = extract_file("src/f2.py", "from . import f1\nf1.alive()\n");
    let f_test = extract_file("src/test_f1.py", "def test_something(): pass\n");
    let f_route = extract_file("src/route.py", "@app.route('/')\ndef handle(): pass\n");
    let f_pb2 = extract_file(
        "src/api_pb2.py",
        "# Generated by the protocol buffer compiler.  DO NOT EDIT!\ndef some_func(): pass\n",
    );

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[
        f1.clone(),
        f2.clone(),
        f_test.clone(),
        f_route.clone(),
        f_pb2.clone(),
    ]);
    let resolution = resolver.resolve_all(&[
        f1.clone(),
        f2.clone(),
        f_test.clone(),
        f_route.clone(),
        f_pb2.clone(),
    ]);

    let analysis = analyze(&[f1, f2, f_test, f_route, f_pb2], &resolution);

    // Python function not called and not exported -> dead (confidence 0.9)
    let dead_sym = analysis
        .dead_symbols
        .iter()
        .find(|l| l.symbol_name == "dead" && l.file_path == "src/f1.py")
        .unwrap();
    assert!(!dead_sym.is_exempt);
    assert!(dead_sym.confidence >= 0.9);

    // Python function called by another file -> alive
    let alive_sym = analysis
        .dead_symbols
        .iter()
        .find(|l| l.symbol_name == "alive" && l.file_path == "src/f1.py");
    assert!(alive_sym.is_none(), "called function should not be dead");

    // Python function in test file -> exempt
    let test_sym = analysis
        .dead_symbols
        .iter()
        .find(|l| l.symbol_name == "test_something" && l.file_path == "src/test_f1.py")
        .unwrap();
    assert!(test_sym.is_exempt);
    assert!(test_sym.confidence < 0.5); // Should be 0.3 based on specs

    // Function in file with @app.route -> exempt
    let route_sym = analysis
        .dead_symbols
        .iter()
        .find(|l| l.symbol_name == "handle" && l.file_path == "src/route.py")
        .unwrap();
    assert!(route_sym.is_exempt);

    // Function in generated _pb2.py -> exempt
    let pb2_sym = analysis
        .dead_symbols
        .iter()
        .find(|l| l.symbol_name == "some_func" && l.file_path == "src/api_pb2.py")
        .unwrap();
    assert!(pb2_sym.is_exempt);
}

#[test]
fn test_liveness_is_scoped_to_the_target_file() {
    let first = extract_file("src/first.py", "def helper(): pass\n");
    let second = extract_file("src/second.py", "def helper(): pass\n");
    let caller = extract_file("src/caller.py", "def use(): pass\n");
    let resolution = devmap_resolve::model::ResolutionResult {
        edges: vec![devmap_resolve::model::ResolvedEdge {
            source_file: caller.file_path.clone(),
            target_file: first.file_path.clone(),
            source_symbol: "use".to_string(),
            target_symbol: "helper".to_string(),
            edge_kind: EdgeKind::Calls,
            confidence: Confidence::DETERMINISTIC,
            resolution: None,
            details: None,
        }],
        receiver_types: std::collections::BTreeMap::new(),
        reexport_chains: std::collections::BTreeMap::new(),
        unresolved: Vec::new(),
    };

    let report = analyze_liveness(&[first, second, caller], &resolution);

    assert!(
        !report
            .iter()
            .any(|item| item.file_path == "src/first.py" && item.symbol_name == "helper"),
        "the called helper should be live"
    );
    assert!(
        report
            .iter()
            .any(|item| item.file_path == "src/second.py" && item.symbol_name == "helper"),
        "an identically named helper in another file must remain a dead candidate"
    );
}

#[test]
fn test_ambiguous_calls_do_not_prove_a_candidate_live() {
    let first = extract_file("pkg/a.py", "def process(): pass\n");
    let second = extract_file("pkg/b.py", "def process(): pass\n");
    let caller = extract_file("main.py", "def run(): process()\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[first.clone(), second.clone(), caller.clone()]);
    let resolution = resolver.resolve_all(&[first.clone(), second.clone(), caller]);
    let reports = analyze_liveness(&[first, second], &resolution);

    assert!(reports.iter().any(|report| {
        report.file_path == "pkg/a.py" && report.symbol_name == "process" && !report.is_exempt
    }));
    assert!(reports.iter().any(|report| {
        report.file_path == "pkg/b.py" && report.symbol_name == "process" && !report.is_exempt
    }));
}

#[test]
fn test_community_reports_are_deterministic() {
    let extractions = vec![
        extract_file("zeta.py", "def zeta(): pass\n"),
        extract_file("alpha.py", "def alpha(): pass\n"),
        extract_file("middle.py", "def middle(): pass\n"),
        extract_file("isolated.py", "def isolated(): pass\n"),
    ];
    let resolution = devmap_resolve::model::ResolutionResult {
        edges: vec![
            devmap_resolve::model::ResolvedEdge {
                source_file: "zeta.py".to_string(),
                target_file: "alpha.py".to_string(),
                source_symbol: "zeta".to_string(),
                target_symbol: "alpha".to_string(),
                edge_kind: EdgeKind::Calls,
                confidence: Confidence::DETERMINISTIC,
                resolution: None,
                details: None,
            },
            devmap_resolve::model::ResolvedEdge {
                source_file: "alpha.py".to_string(),
                target_file: "middle.py".to_string(),
                source_symbol: "alpha".to_string(),
                target_symbol: "middle".to_string(),
                edge_kind: EdgeKind::Calls,
                confidence: Confidence::DETERMINISTIC,
                resolution: None,
                details: None,
            },
        ],
        receiver_types: std::collections::BTreeMap::new(),
        reexport_chains: std::collections::BTreeMap::new(),
        unresolved: Vec::new(),
    };

    let expected =
        serde_json::to_string(&detect_communities(&extractions, &resolution).communities).unwrap();
    for _ in 0..16 {
        let actual =
            serde_json::to_string(&detect_communities(&extractions, &resolution).communities)
                .unwrap();
        assert_eq!(
            actual, expected,
            "community output changed between identical runs"
        );
    }
}

#[test]
fn test_incremental_store() -> anyhow::Result<()> {
    let store = Store::open_in_memory()?;

    // Gen 1: 100 files
    let mut exts = Vec::new();
    for i in 0..100 {
        exts.push(extract_file(
            &format!("src/f{}.py", i),
            &format!("def fn{}(): pass", i),
        ));
    }

    let mut resolver = Resolver::new();
    resolver.index_extractions(&exts);
    let resolution = resolver.resolve_all(&exts);
    let analysis = analyze(&exts, &resolution);

    let _gen1 = store.save_generation(&exts, &resolution, &analysis)?;

    // Gen 2: edit 1 file (f0.py), delete 1 file (f1.py), rename 1 file (f2.py -> f999.py)
    let mut exts2 = exts.clone();

    // Edit f0.py
    exts2[0] = extract_file("src/f0.py", "def fn0_edited(): pass");

    // Delete f1.py (remove from array)
    exts2.remove(1); // was f1

    // Rename f2.py (now at index 1 after removal) to f999.py
    exts2[1] = extract_file("src/f999.py", "def fn999(): pass");

    let mut res2 = Resolver::new();
    res2.index_extractions(&exts2);
    let resolution2 = res2.resolve_all(&exts2);
    let analysis2 = analyze(&exts2, &resolution2);

    let opts = GenerationWriteOpts {
        affected_paths: vec!["src/f0.py".to_string(), "src/f999.py".to_string()],
        deleted_paths: vec!["src/f1.py".to_string(), "src/f2.py".to_string()],
        build_started: None,
        repo_root: None,
        discovery_refusals: None,
    };

    let gen2 = store.save_generation_with_opts(&exts2, &resolution2, &analysis2, opts)?;

    // Validate contents of Gen 2 using list_generation_paths
    let gen2_paths = store.list_generation_paths(gen2)?;
    assert!(
        !gen2_paths.contains(&"src/f1.py".to_string()),
        "Deleted file should be absent"
    );
    assert!(
        gen2_paths.contains(&"src/f999.py".to_string()),
        "Renamed file should be present"
    );
    assert!(
        !gen2_paths.contains(&"src/f2.py".to_string()),
        "Old path of renamed file should be absent"
    );
    assert!(
        gen2_paths.contains(&"src/f0.py".to_string()),
        "Edited file should be present"
    );

    Ok(())
}

#[test]
fn test_fts_query() -> anyhow::Result<()> {
    let store = Store::open_in_memory()?;

    let ext1 = extract_file(
        "src/query_target.py",
        "def my_special_function(): pass\nclass MySpecialClass: pass",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext1));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext1));
    let analysis = analyze(std::slice::from_ref(&ext1), &resolution);

    store.save_generation(std::slice::from_ref(&ext1), &resolution, &analysis)?;
    store.repair_fts()?;

    // Engine search test
    let exts = [ext1];
    let engine = QueryEngine::new(&exts, &resolution);
    let req = Request {
        query: "special".to_string(),
        token_budget: 1000,
        min_confidence: 0.0,
        max_depth: 1,
    };
    let results = engine.search(req);

    assert!(results.total >= 2);
    assert_eq!(results.shown as usize, results.items.len());

    for r in &results.items {
        println!("Hit: {} {}", r.symbol_name, r.file_path);
    }
    let has_func = results.items.iter().any(|r| {
        r.symbol_name.contains("my_special_function") && r.file_path == "src/query_target.py"
    });
    let has_class = results
        .items
        .iter()
        .any(|r| r.symbol_name.contains("MySpecialClass") && r.file_path == "src/query_target.py");

    assert!(has_func, "Failed has_func");
    assert!(has_class, "Failed has_class");

    // Store search FTS test
    let fts_results = store.search_fts("special", 50)?;
    assert!(fts_results
        .iter()
        .any(|(name, _, _)| name == "my_special_function"));

    Ok(())
}

#[test]
fn test_stress_scale() -> anyhow::Result<()> {
    let mut exts = Vec::new();
    for i in 0..2000 {
        let code = format!(
            "import stress{}\ndef fn{}():\n    stress{}.fn{}()\n",
            (i + 1) % 2000,
            i,
            (i + 1) % 2000,
            (i + 1) % 2000
        );
        exts.push(extract_file(&format!("src/stress{}.py", i), &code));
    }

    let start = Instant::now();

    let mut resolver = Resolver::new();
    resolver.index_extractions(&exts);
    let resolution = resolver.resolve_all(&exts);

    let analysis = analyze(&exts, &resolution);

    let store = Store::open_in_memory()?;
    store.save_generation(&exts, &resolution, &analysis)?;

    let elapsed = start.elapsed();

    // Debug-mode ratchet with margin over the measured sub-second local run.
    assert!(
        elapsed.as_secs_f32() < 5.0,
        "Stress test took too long: {:?}",
        elapsed
    );

    // Community detection check
    let mut found_communities = false;
    for comm in &analysis.communities {
        if comm.members.len() > 1 {
            found_communities = true;
            break;
        }
    }
    assert!(found_communities, "Expected to find some communities");

    // Dead code check - all functions should be alive since they call each other in a ring
    let dead: Vec<_> = analysis
        .dead_symbols
        .iter()
        .filter(|report| !report.is_exempt)
        .map(|report| (&report.file_path, &report.symbol_name))
        .collect();
    assert!(
        dead.is_empty(),
        "ring calls must keep every function live: {dead:?}"
    );

    Ok(())
}
