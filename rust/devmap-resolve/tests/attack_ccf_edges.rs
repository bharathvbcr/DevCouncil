//! Adversarial round on the resolver changes of lane compiler-cheap-fixes
//! (ec4283f0): a Rust/Go type construction is a `References` edge, and a Go
//! call names the set of build-constraint variants.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

const GOMOD: &str = "module example.com/app\n\ngo 1.22\n";

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    let module = devmap_extract::parse_go_mod("go.mod", GOMOD).expect("go.mod parses");
    resolver.index_go_modules(std::slice::from_ref(&module));
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn edges_from<'a>(result: &'a ResolutionResult, caller: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| edge.source_symbol.rsplit("::").next() == Some(caller))
        .collect()
}

fn describe(edges: &[&ResolvedEdge]) -> Vec<String> {
    let mut rows: Vec<String> = edges
        .iter()
        .map(|e| {
            format!(
                "{:?} -> {} [{}]",
                e.edge_kind,
                e.target_symbol,
                e.details.as_deref().unwrap_or("")
            )
        })
        .collect();
    rows.sort();
    rows
}

/// The edge `caller` has to `target` that is either a `References` or a
/// `Constructor` use, and no `Calls` edge to a type.
fn assert_constructs(result: &ResolutionResult, caller: &str, target: &str) {
    let edges = edges_from(result, caller);
    let to_target: Vec<&&ResolvedEdge> = edges
        .iter()
        .filter(|e| e.target_symbol.ends_with(target))
        .collect();
    assert!(
        to_target
            .iter()
            .any(|e| e.edge_kind == EdgeKind::References),
        "{caller} constructs {target}: expected a References edge, got {:?}",
        describe(&edges)
    );
    assert!(
        to_target.iter().all(|e| e.edge_kind != EdgeKind::Calls),
        "{caller} -> {target} must not be a Calls edge: {:?}",
        describe(&edges)
    );
}

// ---- item 5: the tuple-struct `Wrapper(Store)` case ------------------------

#[test]
fn rust_tuple_struct_wrapper_of_a_unit_struct_is_referenced() {
    let src = "pub struct Store;\n\
pub struct Wrapper(Store);\n\
pub fn make() -> Wrapper { Wrapper(Store) }\n";
    let result = resolve(&[("src/lib.rs", src)]);
    assert_constructs(&result, "make", "Wrapper");
}

#[test]
fn rust_tuple_struct_wrapper_of_a_constructor_call_is_referenced() {
    let src = "pub struct Store { n: u8 }\n\
impl Store { pub fn new() -> Store { Store { n: 0 } } }\n\
pub struct Wrapper(Store);\n\
pub fn make() -> Wrapper { Wrapper(Store::new()) }\n";
    let result = resolve(&[("src/lib.rs", src)]);
    assert_constructs(&result, "make", "Wrapper");
    // The inner call is still a call.
    let edges = edges_from(&result, "make");
    assert!(
        edges
            .iter()
            .any(|e| e.edge_kind == EdgeKind::Calls && e.target_symbol.ends_with("new")),
        "{:?}",
        describe(&edges)
    );
}

#[test]
fn rust_tuple_struct_wrapper_across_files_is_referenced() {
    let a = "pub struct Store;\npub struct Wrapper(pub Store);\n";
    let b = "use crate::a::{Store, Wrapper};\n\
pub fn make(s: Store) -> Wrapper { Wrapper(s) }\n";
    let lib = "mod a;\nmod b;\n";
    let result = resolve(&[("src/lib.rs", lib), ("src/a.rs", a), ("src/b.rs", b)]);
    assert_constructs(&result, "make", "Wrapper");
}

#[test]
fn rust_tuple_struct_wrapper_through_an_alias_is_referenced() {
    let a = "pub struct Store;\npub struct Wrapper(pub Store);\n";
    let b = "use crate::a::{Store, Wrapper as W};\n\
pub fn make(s: Store) -> W { W(s) }\n";
    let lib = "mod a;\nmod b;\n";
    let result = resolve(&[("src/lib.rs", lib), ("src/a.rs", a), ("src/b.rs", b)]);
    assert_constructs(&result, "make", "Wrapper");
}

#[test]
fn rust_tuple_struct_wrapper_by_module_path_is_referenced() {
    let a = "pub struct Store;\npub struct Wrapper(pub Store);\n";
    let b = "pub fn make(s: crate::a::Store) -> crate::a::Wrapper { crate::a::Wrapper(s) }\n";
    let lib = "mod a;\nmod b;\n";
    let result = resolve(&[("src/lib.rs", lib), ("src/a.rs", a), ("src/b.rs", b)]);
    assert_constructs(&result, "make", "Wrapper");
}

#[test]
fn rust_tuple_struct_self_construction_is_referenced() {
    let src = "pub struct Store;\n\
pub struct Wrapper(Store);\n\
impl Wrapper { pub fn wrap(s: Store) -> Self { Self(s) } }\n";
    let result = resolve(&[("src/lib.rs", src)]);
    let edges = edges_from(&result, "wrap");
    assert!(
        edges.iter().all(|e| !(e.edge_kind == EdgeKind::Calls
            && e.target_symbol.ends_with("Wrapper"))),
        "Self(..) must not be a Calls edge to the type: {:?}",
        describe(&edges)
    );
}

#[test]
fn rust_generic_tuple_struct_is_referenced() {
    let src = "pub struct Store;\n\
pub struct Wrapper<T>(T);\n\
pub fn make() -> Wrapper<Store> { Wrapper::<Store>(Store) }\n\
pub fn infer() -> Wrapper<Store> { Wrapper(Store) }\n";
    let result = resolve(&[("src/lib.rs", src)]);
    assert_constructs(&result, "infer", "Wrapper");
    assert_constructs(&result, "make", "Wrapper");
}

/// A struct and a function of one name in two modules of one file: the call to
/// the function must stay a `Calls` edge, and the construction of the struct
/// must not become one.
#[test]
fn rust_same_name_struct_and_function_in_two_modules_keep_their_kinds() {
    let src = "mod a { pub struct Foo(pub i32); pub fn build() -> Foo { Foo(1) } }\n\
mod b { pub fn Foo() -> i32 { 1 } pub fn run() -> i32 { Foo() } }\n";
    let result = resolve(&[("src/lib.rs", src)]);
    let run = edges_from(&result, "run");
    assert!(
        run.iter()
            .any(|e| e.edge_kind == EdgeKind::Calls && e.target_symbol.ends_with("Foo")),
        "run() calls the function Foo: {:?}",
        describe(&run)
    );
    let build = edges_from(&result, "build");
    assert!(
        build
            .iter()
            .all(|e| !(e.edge_kind == EdgeKind::Calls && e.target_symbol.ends_with("Foo"))),
        "build() constructs the struct Foo, which is not a call: {:?}",
        describe(&build)
    );
}

// ---- item 4: Go build-constraint variants ----------------------------------

/// A caller inside one variant file reaches only that file's declaration: the
/// other variant is excluded from every build that includes this file.
#[test]
fn go_a_caller_in_a_variant_file_does_not_reach_the_other_os_variant() {
    let linux = "package p\n\nfunc helper() {}\n\nfunc Run() { helper() }\n";
    let windows = "package p\n\nfunc helper() {}\n";
    let result = resolve(&[("p/run_linux.go", linux), ("p/run_windows.go", windows)]);
    let run = edges_from(&result, "Run");
    let calls: Vec<_> = run
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls)
        .collect();
    assert!(
        calls.iter().all(|e| e.target_file == "p/run_linux.go"),
        "Run is in run_linux.go; run_windows.go is never compiled with it: {:?}",
        describe(&run)
    );
}

/// A bare call from an unconstrained file reaches both variants, and a local
/// function value of the same name shadows them.
#[test]
fn go_a_parameter_named_like_the_variants_shadows_them() {
    let unix = "//go:build unix\n\npackage p\n\nfunc Configure() {}\n";
    let other = "//go:build !unix\n\npackage p\n\nfunc Configure() {}\n";
    let caller = "package p\n\nfunc Use(Configure func()) { Configure() }\n";
    let result = resolve(&[
        ("p/c_unix.go", unix),
        ("p/c_other.go", other),
        ("p/use.go", caller),
    ]);
    let edges = edges_from(&result, "Use");
    assert!(
        edges.iter().all(|e| e.edge_kind != EdgeKind::Calls),
        "Configure is the parameter, not either package function: {:?}",
        describe(&edges)
    );
}

/// The same shape with a local variable.
#[test]
fn go_a_local_variable_named_like_the_variants_shadows_them() {
    let unix = "//go:build unix\n\npackage p\n\nfunc Configure() {}\n";
    let other = "//go:build !unix\n\npackage p\n\nfunc Configure() {}\n";
    let caller = "package p\n\nfunc Use() {\n\tConfigure := func() {}\n\tConfigure()\n}\n";
    let result = resolve(&[
        ("p/c_unix.go", unix),
        ("p/c_other.go", other),
        ("p/use.go", caller),
    ]);
    let edges = edges_from(&result, "Use");
    assert!(
        edges.iter().all(|e| e.edge_kind != EdgeKind::Calls),
        "Configure is the local, not either package function: {:?}",
        describe(&edges)
    );
}

/// Two `//go:build ignore` programs in one directory are separate programs, not
/// build variants of one another: a caller in the third sees neither.
#[test]
fn go_two_ignore_programs_are_not_variants_of_each_other() {
    let a = "//go:build ignore\n\npackage main\n\nfunc write() {}\n\nfunc main() { write() }\n";
    let b = "//go:build ignore\n\npackage main\n\nfunc write() {}\n\nfunc main2() { write() }\n";
    let result = resolve(&[("gen/a.go", a), ("gen/b.go", b)]);
    let main = edges_from(&result, "main");
    assert!(
        main.iter()
            .filter(|e| e.edge_kind == EdgeKind::Calls)
            .all(|e| e.target_file == "gen/a.go"),
        "{:?}",
        describe(&main)
    );
    let main2 = edges_from(&result, "main2");
    assert!(
        main2
            .iter()
            .filter(|e| e.edge_kind == EdgeKind::Calls)
            .all(|e| e.target_file == "gen/b.go"),
        "{:?}",
        describe(&main2)
    );
}

/// A call through an import whose package clause differs from the directory's
/// other package (an `ignore` `package main` generator beside `package proc`)
/// must not make the generator's function a variant.
#[test]
fn go_a_generator_main_beside_a_package_is_not_a_variant() {
    let unix = "//go:build unix\n\npackage proc\n\nfunc Configure() {}\n";
    let other = "//go:build !unix\n\npackage proc\n\nfunc Configure() {}\n";
    let generator = "//go:build ignore\n\npackage main\n\nfunc Configure() {}\n\nfunc main() {}\n";
    let caller =
        "package app\n\nimport \"example.com/app/proc\"\n\nfunc Build() { proc.Configure() }\n";
    let result = resolve(&[
        ("proc/c_unix.go", unix),
        ("proc/c_other.go", other),
        ("proc/gen.go", generator),
        ("app/app.go", caller),
    ]);
    let edges = edges_from(&result, "Build");
    let files: Vec<&str> = edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls)
        .map(|e| e.target_file.as_str())
        .collect();
    assert!(
        !files.contains(&"proc/gen.go"),
        "the ignore generator is another package: {:?}",
        describe(&edges)
    );
}

/// A test-only twin pair must not be reached from a non-test caller.
#[test]
fn go_test_file_variants_are_not_callees_of_non_test_code() {
    let a = "//go:build linux\n\npackage p\n\nfunc fixture() {}\n";
    let b = "//go:build !linux\n\npackage p\n\nfunc fixture() {}\n";
    let caller = "package p\n\nfunc Real() { fixture() }\n";
    let result = resolve(&[
        ("p/a_test.go", a),
        ("p/b_test.go", b),
        ("p/real.go", caller),
    ]);
    let edges = edges_from(&result, "Real");
    assert!(
        edges.iter().all(|e| e.edge_kind != EdgeKind::Calls),
        "{:?}",
        describe(&edges)
    );
}
