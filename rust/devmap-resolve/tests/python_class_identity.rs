//! Python resolves a class by the name the file wrote: its own `class`
//! statement, else the import that bound it. Source roots come from the
//! manifest, or from the one directory that holds a package of the name.
//!
//! The flat `(type, method)` tables the resolver keeps cannot say which of two
//! same-named classes a call means, and a lone hit in the *other* file used to
//! read as the answer.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_extract::pyproject::PythonSourceRoots;
use devmap_extract::ProjectManifests;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve_with(files: &[(&str, &str)], roots: &[(&str, &[&str])]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let manifests = ProjectManifests {
        python_roots: roots
            .iter()
            .map(|(dir, roots)| PythonSourceRoots {
                dir: dir.to_string(),
                roots: roots.iter().map(|root| root.to_string()).collect(),
            })
            .collect(),
        ..ProjectManifests::default()
    };
    let mut resolver = Resolver::new();
    resolver.index_project_manifests(&manifests);
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    resolve_with(files, &[])
}

fn imports(resolution: &ResolutionResult, from: &str, to: &str) -> bool {
    resolution.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Imports && edge.source_file == from && edge.target_file == to
    })
}

fn targets_of(resolution: &ResolutionResult, caller: &str) -> Vec<String> {
    resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == caller)
        .map(|edge| edge.target_symbol.clone())
        .collect()
}

/// The methods a caller reaches: a constructor call also edges to the class
/// itself, and the class tests are about which method a call lands on.
fn methods_of(resolution: &ResolutionResult, caller: &str) -> Vec<String> {
    targets_of(resolution, caller)
        .into_iter()
        .filter(|target| {
            target
                .rsplit("::")
                .next()
                .is_some_and(|tail| tail.contains('.'))
        })
        .collect()
}

const UTIL: &str = "def split(text):\n    return text.split(',')\n";

#[test]
fn a_manifest_source_root_places_an_absolute_import() {
    let resolution = resolve_with(
        &[
            ("lib/pkg/__init__.py", ""),
            ("lib/pkg/util.py", UTIL),
            ("other/util.py", UTIL),
            (
                "tests/test_u.py",
                "from pkg.util import split\n\ndef test_it():\n    return split('a')\n",
            ),
        ],
        &[("", &["lib"])],
    );
    assert!(imports(&resolution, "tests/test_u.py", "lib/pkg/util.py"));
    assert_eq!(
        targets_of(&resolution, "tests/test_u.py::test_it"),
        ["lib/pkg/util.py::split"]
    );
}

#[test]
fn a_manifest_governs_only_its_own_directory() {
    // `svc/` declares `python` as a root; a test outside `svc/` does not see it.
    let files = [
        ("svc/python/svcpkg/__init__.py", ""),
        ("svc/python/svcpkg/util.py", UTIL),
        ("svc/tests/test_in.py", "from svcpkg.util import split\n"),
        ("elsewhere/test_out.py", "from svcpkg.util import split\n"),
    ];
    // Two directories hold no `svcpkg`, so the no-manifest fallback is not
    // what answers the inside importer: the manifest is.
    let resolution = resolve_with(&files[..3], &[("svc", &["svc/python"])]);
    assert!(imports(
        &resolution,
        "svc/tests/test_in.py",
        "svc/python/svcpkg/util.py"
    ));
    let manifestless = resolve(&[files[0], files[1], files[3]]);
    assert!(
        imports(
            &manifestless,
            "elsewhere/test_out.py",
            "svc/python/svcpkg/util.py"
        ),
        "the one directory holding the package still places it without a manifest"
    );
}

/// The no-manifest fallback places an import from the one directory that holds a
/// package of that name. A directory called `logging`, `json` or `typing` is
/// not evidence about `import logging`: the standard library owns the name, so
/// the fallback abstains for it, exactly as it does for a name two directories
/// hold. (The positive control is the test below, same shape, a non-stdlib name.)
#[test]
fn a_repo_directory_named_like_a_stdlib_module_does_not_capture_its_import() {
    for name in ["logging", "json", "typing", "collections", "os"] {
        let init = format!("python/{name}/__init__.py");
        let util = format!("python/{name}/util.py");
        let source = format!("from {name}.util import split\n");
        let resolution = resolve(&[
            (init.as_str(), ""),
            (util.as_str(), UTIL),
            ("tests/test_u.py", source.as_str()),
        ]);
        assert!(
            !imports(&resolution, "tests/test_u.py", &util),
            "a repository directory named `{name}` captured `from {name}.util import ..`"
        );
    }
}

#[test]
fn the_one_directory_holding_a_package_places_it_without_a_manifest() {
    let resolution = resolve(&[
        ("python/rootpkg/__init__.py", ""),
        ("python/rootpkg/util.py", UTIL),
        ("tests/test_u.py", "from rootpkg.util import split\n"),
    ]);
    assert!(imports(
        &resolution,
        "tests/test_u.py",
        "python/rootpkg/util.py"
    ));
}

#[test]
fn a_package_name_two_directories_hold_names_neither() {
    let resolution = resolve(&[
        ("a/shared/__init__.py", ""),
        ("a/shared/util.py", UTIL),
        ("b/shared/__init__.py", ""),
        ("b/shared/util.py", UTIL),
        ("tests/test_u.py", "from shared.util import split\n"),
    ]);
    assert!(!imports(&resolution, "tests/test_u.py", "a/shared/util.py"));
    assert!(!imports(&resolution, "tests/test_u.py", "b/shared/util.py"));
}

#[test]
fn a_declared_root_that_lacks_the_module_binds_nothing() {
    let resolution = resolve_with(
        &[
            ("lib/pkg/__init__.py", ""),
            ("lib/pkg/util.py", UTIL),
            ("tests/test_u.py", "from pkg.missing import split\n"),
        ],
        &[("", &["lib"])],
    );
    assert!(!imports(&resolution, "tests/test_u.py", "lib/pkg/util.py"));
}

const A_ENGINE: &str = "class Engine:\n    def step(self):\n        return 1\n";
const B_ENGINE: &str =
    "class Engine:\n    def step(self):\n        return 2\n\n    def only_b(self):\n        return 3\n";

#[test]
fn a_constructed_receiver_binds_the_class_its_import_names() {
    let resolution = resolve(&[
        ("a.py", A_ENGINE),
        ("b.py", B_ENGINE),
        (
            "use_a.py",
            "from a import Engine\n\ndef run():\n    e = Engine()\n    return e.step()\n",
        ),
        (
            "use_b.py",
            "from b import Engine\n\ndef run():\n    e = Engine()\n    return e.step()\n",
        ),
    ]);
    assert_eq!(
        methods_of(&resolution, "use_a.py::run"),
        ["a.py::Engine.step"]
    );
    assert_eq!(
        methods_of(&resolution, "use_b.py::run"),
        ["b.py::Engine.step"]
    );
}

#[test]
fn a_method_only_a_namesake_declares_is_not_the_imported_class_s() {
    // `a.Engine` has no `only_b`; the lone `only_b` in the corpus is `b.Engine`'s.
    let resolution = resolve(&[
        ("a.py", A_ENGINE),
        ("b.py", B_ENGINE),
        (
            "use_a.py",
            "from a import Engine\n\ndef run():\n    e = Engine()\n    return e.only_b()\n",
        ),
    ]);
    let targets = methods_of(&resolution, "use_a.py::run");
    assert!(
        !targets.iter().any(|target| target.starts_with("b.py::")),
        "bound the other file's namesake: {targets:?}"
    );
}

#[test]
fn the_files_own_class_wins_over_an_imported_namesake() {
    let resolution = resolve(&[
        ("a.py", A_ENGINE),
        (
            "mine.py",
            "from a import Engine as Other\n\nclass Engine:\n    def step(self):\n        return 9\n\ndef run():\n    return Engine().step()\n",
        ),
    ]);
    assert_eq!(
        methods_of(&resolution, "mine.py::run"),
        ["mine.py::Engine.step"]
    );
}

#[test]
fn an_aliased_class_import_and_an_inherited_method_bind_through_the_class() {
    let resolution = resolve(&[
        (
            "core.py",
            "class Base:\n    def quack(self):\n        return 'b'\n\nclass Engine(Base):\n    def step(self):\n        return 1\n",
        ),
        (
            "use.py",
            "from core import Engine as E2\n\ndef run():\n    a = E2().step()\n    return E2().quack()\n",
        ),
    ]);
    let targets = methods_of(&resolution, "use.py::run");
    assert!(
        targets.contains(&"core.py::Engine.step".to_string()),
        "{targets:?}"
    );
    assert!(
        targets.contains(&"core.py::Base.quack".to_string()),
        "{targets:?}"
    );
}

#[test]
fn super_reaches_the_base_declaration_and_follows_the_c3_order_on_two_bases() {
    let resolution = resolve(&[(
        "m.py",
        "class Base:\n    def run(self):\n        return 1\n\nclass Child(Base):\n    def run(self):\n        return super().run() + 1\n\nclass Left:\n    def go(self):\n        return 1\n\nclass Right:\n    def go(self):\n        return 2\n\nclass Both(Left, Right):\n    def go(self):\n        return super().go()\n",
    )]);
    assert_eq!(
        methods_of(&resolution, "m.py::Child.run"),
        ["m.py::Base.run"]
    );
    // `class Both(Left, Right)`: Python's C3 order is Both, Left, Right, so
    // `super().go()` is `Left.go`. (This used to assert no edge, from before
    // the walk modelled the MRO.)
    assert_eq!(
        methods_of(&resolution, "m.py::Both.go"),
        ["m.py::Left.go"],
        "the first base in C3 order that declares it"
    );
}

/// Python says which `Base` a `class Child(Base)` means: the one the file
/// imports. Two files declaring the name does not make the child's base
/// ambiguous, so the inherited method binds to the imported base and never to
/// the namesake. (This test used to demand abstention, which was the flat
/// `supertypes` table's limit rather than a fact about the program.)
#[test]
fn an_inherited_method_through_a_base_two_files_declare_binds_the_imported_one() {
    let resolution = resolve(&[
        ("one.py", "class Base:\n    def quack(self):\n        return 1\n"),
        ("two.py", "class Base:\n    def quack(self):\n        return 2\n"),
        (
            "child.py",
            "from one import Base\n\nclass Child(Base):\n    pass\n\ndef run():\n    return Child().quack()\n",
        ),
    ]);
    assert_eq!(
        methods_of(&resolution, "child.py::run"),
        vec!["one.py::Base.quack".to_string()]
    );
}

/// `import one; class Child(one.Base)`: the qualified name is the class `one`
/// declares, whatever other files call theirs. (The flat table could not say
/// which `Base` this was and this test used to demand abstention.)
#[test]
fn an_inherited_method_through_a_qualified_base_two_files_declare_binds_the_imported_module_s() {
    let resolution = resolve(&[
        ("one.py", "class Base:\n    def quack(self):\n        return 1\n"),
        ("two.py", "class Base:\n    def quack(self):\n        return 2\n"),
        (
            "child.py",
            "import one\n\nclass Child(one.Base):\n    pass\n\ndef run():\n    return Child().quack()\n",
        ),
    ]);
    assert_eq!(
        methods_of(&resolution, "child.py::run"),
        vec!["one.py::Base.quack".to_string()]
    );
}

/// A base declared twice in the very file that names it is ambiguous for real:
/// the import cannot choose, so `super()` abstains instead of binding either.
#[test]
fn super_through_a_base_declared_twice_in_its_own_file_is_not_bound() {
    let resolution = resolve(&[
        (
            "child.py",
            "class Base:\n    def ping(self):\n        return 1\n\nclass Base:\n    def ping(self):\n        return 2\n\nclass Child(Base):\n    def ping(self):\n        return super().ping()\n",
        ),
        ("other.py", "class Base:\n    def ping(self):\n        return 3\n"),
    ]);
    assert!(
        methods_of(&resolution, "child.py::Child.ping").is_empty(),
        "{:?}",
        methods_of(&resolution, "child.py::Child.ping")
    );
}

#[test]
fn a_property_read_and_a_cls_call_and_a_with_item_are_edges() {
    let resolution = resolve(&[
        (
            "core.py",
            "class Engine:\n    @property\n    def size(self):\n        return 1\n\n    @classmethod\n    def create(cls):\n        return cls()\n\n    def __enter__(self):\n        return self\n\n    def __exit__(self, *exc):\n        return None\n",
        ),
        (
            "use.py",
            "from core import Engine\n\ndef props():\n    return Engine().size\n\ndef ctx():\n    with Engine() as e:\n        return e\n",
        ),
    ]);
    assert!(
        resolution.edges.iter().any(|edge| {
            edge.source_symbol == "use.py::props" && edge.target_symbol == "core.py::Engine.size"
        }),
        "property read"
    );
    assert!(
        resolution.edges.iter().any(|edge| {
            edge.source_symbol == "core.py::Engine.create"
                && edge.target_symbol == "core.py::Engine"
        }),
        "cls()"
    );
    assert!(
        methods_of(&resolution, "use.py::ctx").contains(&"core.py::Engine.__enter__".to_string()),
        "with"
    );
}
