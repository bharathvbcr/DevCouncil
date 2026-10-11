//! Kotlin's receivers, packages and interop: what a call written the way
//! Android code writes it binds to, and what it must refuse.
//!
//! The fixture rows in `testdata/accuracy/kotlin*` are the corpus-shaped
//! truth; these pin each mechanism alone so a regression names its cause.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_extract::{GradleSettings, ProjectManifests};
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve_with(files: &[(&str, &str)], manifests: Option<&ProjectManifests>) -> ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    if let Some(manifests) = manifests {
        resolver.index_project_manifests(manifests);
    }
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    resolve_with(files, None)
}

/// `(source, target, resolution kind)` of every call edge.
fn calls(result: &ResolutionResult) -> Vec<(String, String, ResolutionKind)> {
    let mut found: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .filter_map(|edge| {
            Some((
                edge.source_symbol.clone(),
                edge.target_symbol.clone(),
                edge.resolution.as_ref()?.kind(),
            ))
        })
        .collect();
    found.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    found
}

fn has(result: &ResolutionResult, from: &str, to: &str) -> bool {
    calls(result)
        .iter()
        .any(|(source, target, _)| source == from && target == to)
}

#[test]
fn a_constructor_property_and_a_parameter_type_their_receivers() {
    let result = resolve(&[
        (
            "a/Repo.kt",
            "package a\ninterface Dao {\n    fun all(): Int\n}\nclass Api {\n    fun fetch(): Int = 1\n}\n",
        ),
        (
            "a/Impl.kt",
            "package a\n\
             class Impl(private val dao: Dao, val api: Api) {\n\
             \x20   fun load(): Int = dao.all() + api.fetch()\n\
             }\n\
             fun use(api: Api): Int = api.fetch()\n",
        ),
    ]);
    assert!(
        has(&result, "a/Impl.kt::Impl.load", "a/Repo.kt::Dao.all"),
        "{:?}",
        calls(&result)
    );
    assert!(has(&result, "a/Impl.kt::Impl.load", "a/Repo.kt::Api.fetch"));
    assert!(has(&result, "a/Impl.kt::use", "a/Repo.kt::Api.fetch"));
}

#[test]
fn a_constructor_parameter_that_is_not_a_property_is_not_a_member() {
    // Without `val`/`var` the parameter exists only for initialisation, so a
    // method's `dao` is some other binding and must not be typed from it.
    let result = resolve(&[
        ("a/Repo.kt", "package a\nclass Dao { fun all(): Int = 1 }\n"),
        (
            "a/Impl.kt",
            "package a\nclass Impl(dao: Dao) {\n    fun load(dao: Int): Int = dao.all()\n}\n",
        ),
    ]);
    assert!(
        !has(&result, "a/Impl.kt::Impl.load", "a/Repo.kt::Dao.all"),
        "{:?}",
        calls(&result)
    );
}

#[test]
fn a_function_typed_parameter_is_a_local_not_a_namesake_method() {
    let result = resolve(&[(
        "a/E.kt",
        "package a\nclass Widget { fun onStart(n: Int) {} }\nfun runWith(onStart: (Int) -> Unit) {\n    onStart(1)\n}\n",
    )]);
    assert!(
        !has(&result, "a/E.kt::runWith", "a/E.kt::Widget.onStart"),
        "{:?}",
        calls(&result)
    );
}

#[test]
fn a_typed_value_called_like_a_function_is_its_invoke() {
    let result = resolve(&[(
        "a/Op.kt",
        "package a\nclass Op { operator fun invoke(x: Int): Int = x }\n\
         class UseCase(private val op: Op) {\n    fun run(): Int = op(2)\n}\n\
         fun local(): Int {\n    val op = Op()\n    return op(3)\n}\n",
    )]);
    assert!(
        has(&result, "a/Op.kt::UseCase.run", "a/Op.kt::Op.invoke"),
        "{:?}",
        calls(&result)
    );
    assert!(has(&result, "a/Op.kt::local", "a/Op.kt::Op.invoke"));
}

#[test]
fn super_reaches_the_base_method_and_never_the_override_itself() {
    let result = resolve(&[(
        "a/S.kt",
        "package a\nopen class Base { open fun go() {} }\nclass Child : Base() {\n    override fun go() {\n        super.go()\n    }\n}\n",
    )]);
    assert!(
        has(&result, "a/S.kt::Child.go", "a/S.kt::Base.go"),
        "{:?}",
        calls(&result)
    );
    assert!(!has(&result, "a/S.kt::Child.go", "a/S.kt::Child.go"));
    assert!(
        !result
            .edges
            .iter()
            .any(|edge| edge.source_symbol == "a/S.kt::Child.go"
                && edge.target_symbol == "a/S.kt::Child.go"),
        "no edge of any kind, a reference included, joins the override to itself"
    );
}

#[test]
fn a_callable_reference_is_a_call_on_its_receiver() {
    let result = resolve(&[(
        "a/R.kt",
        "package a\nclass Vm { fun onEvent(e: Int) {} }\nfun register(f: (Int) -> Unit) {}\nfun helper(n: Int) {}\n\
         fun wire(vm: Vm, xs: List<Int>) {\n    register(vm::onEvent)\n    xs.forEach(::helper)\n}\n",
    )]);
    assert!(
        has(&result, "a/R.kt::wire", "a/R.kt::Vm.onEvent"),
        "{:?}",
        calls(&result)
    );
    assert!(has(&result, "a/R.kt::wire", "a/R.kt::helper"));
}

#[test]
fn an_import_resolves_by_package_not_by_file_name() {
    let result = resolve(&[
        (
            "lib/src/main/java/p/q/Fmt.kt",
            "package p.q\nfun format(n: Int): String = n.toString()\nfun String.shout(): String = this\n",
        ),
        (
            "app/src/main/java/x/Use.kt",
            "package x\nimport p.q.format as fmt\nimport p.q.shout\n\
             fun useAlias(): String = fmt(3)\nfun lit(): String = \"a\".shout()\n",
        ),
    ]);
    assert!(
        calls(&result).iter().any(|(source, target, kind)| source
            == "app/src/main/java/x/Use.kt::useAlias"
            && target == "lib/src/main/java/p/q/Fmt.kt::format"
            && *kind == ResolutionKind::ImportScoped),
        "an aliased top-level function binds by its original name: {:?}",
        calls(&result)
    );
    assert!(
        has(
            &result,
            "app/src/main/java/x/Use.kt::lit",
            "lib/src/main/java/p/q/Fmt.kt::String.shout"
        ),
        "a literal receiver joins an imported extension: {:?}",
        calls(&result)
    );
}

#[test]
fn a_type_of_the_callers_own_package_needs_no_import_even_across_source_sets() {
    let result = resolve(&[
        (
            "app/src/main/java/com/fx/Registry.kt",
            "package com.fx\nobject Registry { fun register(n: String) {} }\n",
        ),
        (
            "app/src/test/java/com/fx/RegistryTest.kt",
            "package com.fx\nclass RegistryTest {\n    fun t() {\n        Registry.register(\"a\")\n    }\n}\n",
        ),
    ]);
    assert!(
        has(
            &result,
            "app/src/test/java/com/fx/RegistryTest.kt::RegistryTest.t",
            "app/src/main/java/com/fx/Registry.kt::Registry.register"
        ),
        "{:?}",
        calls(&result)
    );
}

#[test]
fn a_nested_class_named_through_its_enclosing_type_is_a_constructor_call() {
    let result = resolve(&[
        (
            "a/Res.kt",
            "package a\nsealed class Res {\n    class Ok(val v: Int) : Res()\n}\n",
        ),
        ("a/Use.kt", "package a\nfun mk(): Res = Res.Ok(1)\n"),
    ]);
    assert!(
        has(&result, "a/Use.kt::mk", "a/Res.kt::Res.Ok"),
        "{:?}",
        calls(&result)
    );
}

#[test]
fn a_factory_return_type_types_a_property_that_names_no_type() {
    let result = resolve(&[(
        "a/C.kt",
        "package a\nclass Clock {\n    fun elapsed(n: Int): Int = n\n    companion object {\n        fun create(): Clock = Clock()\n    }\n}\n\
         class Owner {\n    private val eager = Clock.create()\n    private val lazy by lazy { Clock.create() }\n\
         \x20   fun f() {\n        eager.elapsed(1)\n        lazy.elapsed(2)\n    }\n}\n",
    )]);
    assert!(
        has(&result, "a/C.kt::Owner.f", "a/C.kt::Clock.elapsed"),
        "{:?}",
        calls(&result)
    );
}

#[test]
fn a_call_through_an_interface_reaches_its_implementers_as_dispatch() {
    let result = resolve(&[(
        "a/D.kt",
        "package a\ninterface Repo {\n    fun load(): Int\n}\n\
         class SqlRepo : Repo {\n    override fun load(): Int = 1\n}\n\
         class MemRepo : Repo {\n    override fun load(): Int = 2\n}\n\
         class UseCase(private val repo: Repo) {\n    fun run(): Int = repo.load()\n}\n",
    )]);
    let found = calls(&result);
    assert!(
        found.contains(&(
            "a/D.kt::UseCase.run".to_string(),
            "a/D.kt::Repo.load".to_string(),
            ResolutionKind::ReceiverType
        )),
        "the interface method is a symbol with a direct caller: {found:?}"
    );
    for implementer in ["a/D.kt::SqlRepo.load", "a/D.kt::MemRepo.load"] {
        assert!(
            found.contains(&(
                "a/D.kt::UseCase.run".to_string(),
                implementer.to_string(),
                ResolutionKind::Dispatch
            )),
            "{implementer} is a possible runtime target: {found:?}"
        );
    }
    assert!(result.edges.iter().any(|edge| matches!(
        edge.resolution.as_deref(),
        Some(Resolution::Dispatch { candidates, .. }) if candidates.len() == 2
    )));
}

#[test]
fn a_call_through_an_interface_with_too_many_implementers_is_one_ledger_row() {
    let mut source = String::from("package a\ninterface Repo {\n    fun load(): Int\n}\n");
    for index in 0..20 {
        source.push_str(&format!(
            "class R{index} : Repo {{\n    override fun load(): Int = {index}\n}}\n"
        ));
    }
    source
        .push_str("class UseCase(private val repo: Repo) {\n    fun run(): Int = repo.load()\n}\n");
    let result = resolve(&[("a/D.kt", source.as_str())]);
    let dispatch_edges = calls(&result)
        .into_iter()
        .filter(|(_, _, kind)| *kind == ResolutionKind::Dispatch)
        .count();
    assert_eq!(
        dispatch_edges, 0,
        "past the cap the candidates are not edges"
    );
    assert!(
        result.unresolved.iter().any(|row| row.callee_name == "load"
            && matches!(row.resolution, Resolution::Dispatch { ref candidates, .. } if candidates.len() == 20)),
        "and the whole candidate list is kept on one ledger row"
    );
    assert!(
        has(&result, "a/D.kt::UseCase.run", "a/D.kt::Repo.load"),
        "the direct edge stays"
    );
}

#[test]
fn a_bare_call_inside_apply_on_an_external_receiver_is_not_a_project_namesake() {
    let result = resolve(&[(
        "a/E.kt",
        "package a\nclass Prefs { fun putBoolean(k: String, v: Boolean) {} }\n\
         fun save(editor: android.content.SharedPreferences.Editor) {\n    editor.apply { putBoolean(\"k\", true) }\n}\n",
    )]);
    assert!(
        !has(&result, "a/E.kt::save", "a/E.kt::Prefs.putBoolean"),
        "the editor may declare putBoolean, and the index cannot say it does not: {:?}",
        calls(&result)
    );
}

#[test]
fn kotlin_and_java_name_each_others_members_through_package_headers() {
    let result = resolve(&[
        (
            "app/src/main/java/com/fx/legacy/LegacyMath.java",
            "package com.fx.legacy;\npublic final class LegacyMath {\n    public static int twice(int x) { return x * 2; }\n    public static void back() { com.fx.app.Registry.INSTANCE.register(\"j\"); }\n}\n",
        ),
        (
            "app/src/main/java/com/fx/app/Registry.kt",
            "package com.fx.app\nobject Registry { fun register(n: String) {} }\n",
        ),
        (
            "app/src/main/java/com/fx/app/Main.kt",
            "package com.fx.app\nimport com.fx.legacy.LegacyMath\nfun go(): Int = LegacyMath.twice(2)\n",
        ),
    ]);
    assert!(
        has(
            &result,
            "app/src/main/java/com/fx/app/Main.kt::go",
            "app/src/main/java/com/fx/legacy/LegacyMath.java::LegacyMath.twice"
        ),
        "{:?}",
        calls(&result)
    );
    assert!(
        has(
            &result,
            "app/src/main/java/com/fx/legacy/LegacyMath.java::LegacyMath.back",
            "app/src/main/java/com/fx/app/Registry.kt::Registry.register"
        ),
        "{:?}",
        calls(&result)
    );
}

#[test]
fn a_gradle_module_names_the_source_root_an_import_is_probed_under() {
    // `Thing` sits under a module's own source set, in a file named for it and
    // in a package whose header the importer cannot guess from a directory.
    let files = [
        (
            "libs/util/src/main/kotlin/org/acme/Thing.kt",
            "package org.acme\nclass Thing { fun make(): Int = 1 }\n",
        ),
        (
            "app/src/main/kotlin/Use.kt",
            "import org.acme.Thing\nfun use(): Int = Thing().make()\n",
        ),
    ];
    let manifests = ProjectManifests {
        gradle_modules: vec![GradleSettings {
            dir: String::new(),
            modules: vec!["app".to_string(), "libs/util".to_string()],
        }],
        ..Default::default()
    };
    let result = resolve_with(&files, Some(&manifests));
    assert!(
        result
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Imports
                && edge.target_file == "libs/util/src/main/kotlin/org/acme/Thing.kt"),
        "the import binds to the module's file: {:?}",
        result
            .edges
            .iter()
            .map(|e| (&e.source_symbol, &e.target_file))
            .collect::<Vec<_>>()
    );
}
