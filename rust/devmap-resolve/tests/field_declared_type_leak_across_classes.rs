//! Regression tests for field declared type leaking across classes in one file.
//! Task: ft-d69f8f97c1ca87415c147ac99b636b3c

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

fn calls_from(files: &[(&str, &str)], source: &str) -> Vec<String> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges
        .into_iter()
        .filter(|edge| edge.source_symbol == source && format!("{:?}", edge.edge_kind) == "Calls")
        .map(|edge| edge.target_symbol)
        .collect()
}

/// Swift: FlHolderA declares typed property `var engine: FlEngine`.
/// FlHolderC declares factory-assigned property `var engine = makeFlOther()`.
/// FlHolderC.go must NOT bind `engine.run()` to `FlEngine.run`.
#[test]
fn swift_factory_field_does_not_leak_sibling_declared_type() {
    let source = "struct FlEngine {\n\
         \x20   func run() -> Int { 1 }\n\
         }\n\
         struct FlOther {\n\
         \x20   func run() -> Int { 2 }\n\
         }\n\
         func makeFlOther() -> FlOther { FlOther() }\n\
         struct FlHolderA {\n\
         \x20   var engine: FlEngine\n\
         \x20   func go() -> Int { engine.run() }\n\
         }\n\
         struct FlHolderC {\n\
         \x20   var engine = makeFlOther()\n\
         \x20   func go() -> Int { engine.run() }\n\
         }\n";
    let targets = calls_from(
        &[("FieldLeak.swift", source)],
        "FieldLeak.swift::FlHolderC.go",
    );
    assert!(
        !targets
            .iter()
            .any(|target| target.ends_with("FlEngine.run")),
        "Swift factory field engine in FlHolderC must not leak to FlEngine.run, got {targets:?}"
    );
}

/// Kotlin: FlHolderA declares typed property `val engine: FlEngine = FlEngine()`.
/// FlHolderC declares factory-assigned property `val engine = makeFlOther()`.
/// FlHolderC.go must NOT bind `engine.tick()` to `FlEngine.tick`.
#[test]
fn kotlin_factory_field_does_not_leak_sibling_declared_type() {
    let source = "class FlEngine {\n\
         \x20   fun tick(): Int { return 1 }\n\
         }\n\
         class FlOther {\n\
         \x20   fun tick(): Int { return 2 }\n\
         }\n\
         fun makeFlOther(): FlOther = FlOther()\n\
         class FlHolderA {\n\
         \x20   val engine: FlEngine = FlEngine()\n\
         \x20   fun go(): Int { return engine.tick() }\n\
         }\n\
         class FlHolderC {\n\
         \x20   val engine = makeFlOther()\n\
         \x20   fun go(): Int { return engine.tick() }\n\
         }\n";
    let targets = calls_from(&[("FieldLeak.kt", source)], "FieldLeak.kt::FlHolderC.go");
    assert!(
        !targets.iter().any(|target| target.ends_with("FlEngine.tick")),
        "Kotlin factory property engine in FlHolderC must not leak to FlEngine.tick, got {targets:?}"
    );
}

/// Python: ClassA declares typed field `engine: FlEngine`.
/// ClassC declares factory-assigned field `self.engine = make_other()`.
/// ClassC.go must NOT bind `self.engine.run()` to `FlEngine.run`.
#[test]
fn python_factory_field_does_not_leak_sibling_declared_type() {
    let source = "class FlEngine:\n\
         \x20   def run(self):\n\
         \x20       return 1\n\
         class FlOther:\n\
         \x20   def run(self):\n\
         \x20       return 2\n\
         def make_other():\n\
         \x20   return FlOther()\n\
         class FlHolderA:\n\
         \x20   engine: FlEngine\n\
         \x20   def go(self):\n\
         \x20       return self.engine.run()\n\
         class FlHolderC:\n\
         \x20   def __init__(self):\n\
         \x20       self.engine = make_other()\n\
         \x20   def go(self):\n\
         \x20       return self.engine.run()\n";
    let targets = calls_from(&[("FieldLeak.py", source)], "FieldLeak.py::FlHolderC.go");
    assert!(
        !targets
            .iter()
            .any(|target| target.ends_with("FlEngine.run")),
        "Python field engine in FlHolderC must not leak to FlEngine.run, got {targets:?}"
    );
}

/// TypeScript: HolderA declares typed field `engine: FlEngine`.
/// HolderC declares factory-assigned field `engine = makeOther()`.
/// HolderC.go must NOT bind `this.engine.run()` to `FlEngine.run`.
#[test]
fn typescript_factory_field_does_not_leak_sibling_declared_type() {
    let source = "class FlEngine {\n\
         \x20   run(): number { return 1; }\n\
         }\n\
         class FlOther {\n\
         \x20   run(): number { return 2; }\n\
         }\n\
         function makeOther(): FlOther { return new FlOther(); }\n\
         class FlHolderA {\n\
         \x20   engine: FlEngine;\n\
         \x20   go(): number { return this.engine.run(); }\n\
         }\n\
         class FlHolderC {\n\
         \x20   engine = makeOther();\n\
         \x20   go(): number { return this.engine.run(); }\n\
         }\n";
    let targets = calls_from(&[("FieldLeak.ts", source)], "FieldLeak.ts::FlHolderC.go");
    assert!(
        !targets
            .iter()
            .any(|target| target.ends_with("FlEngine.run")),
        "TypeScript field engine in FlHolderC must not leak to FlEngine.run, got {targets:?}"
    );
}

/// Module-level variable: a true file-level variable is still accessible.
#[test]
fn module_level_variable_remains_accessible() {
    let source = "struct GlobalService {\n\
         \x20   func ping() -> Int { 42 }\n\
         }\n\
         var svc: GlobalService = GlobalService()\n\
         func fileHelper() -> Int {\n\
         \x20   svc.ping()\n\
         }\n\
         struct Worker {\n\
         \x20   func go() -> Int {\n\
         \x20       svc.ping()\n\
         \x20   }\n\
         }\n";
    let targets = calls_from(&[("Global.swift", source)], "Global.swift::Worker.go");
    assert!(
        targets
            .iter()
            .any(|target| target.ends_with("GlobalService.ping")),
        "Worker.go should reach file-level svc.ping, got {targets:?}"
    );
}

#[test]
fn nested_class_field_isolation() {
    let source = "class TargetOuter {\n\
         \x20   fun outerRun(): Int { return 1 }\n\
         }\n\
         class TargetInner {\n\
         \x20   fun innerRun(): Int { return 2 }\n\
         }\n\
         class Outer {\n\
         \x20   val handler: TargetOuter = TargetOuter()\n\
         \x20   class Inner {\n\
         \x20       val handler: TargetInner = TargetInner()\n\
         \x20       fun go(): Int { return handler.innerRun() }\n\
         \x20   }\n\
         \x20   fun go(): Int { return handler.outerRun() }\n\
         }\n";
    let inner_calls = calls_from(&[("Nested.kt", source)], "Nested.kt::Inner.go");
    assert!(
        inner_calls
            .iter()
            .any(|t| t.ends_with("TargetInner.innerRun")),
        "Inner.go should bind TargetInner.innerRun, got {inner_calls:?}"
    );
    assert!(
        !inner_calls
            .iter()
            .any(|t| t.ends_with("TargetOuter.outerRun")),
        "Inner.go must not bind TargetOuter.outerRun, got {inner_calls:?}"
    );
    let outer_calls = calls_from(&[("Nested.kt", source)], "Nested.kt::Outer.go");
    assert!(
        outer_calls
            .iter()
            .any(|t| t.ends_with("TargetOuter.outerRun")),
        "Outer.go should bind TargetOuter.outerRun, got {outer_calls:?}"
    );
    assert!(
        !outer_calls
            .iter()
            .any(|t| t.ends_with("TargetInner.innerRun")),
        "Outer.go must not bind TargetInner.innerRun, got {outer_calls:?}"
    );
}
