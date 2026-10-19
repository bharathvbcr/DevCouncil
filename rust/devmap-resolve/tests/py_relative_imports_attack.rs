//! Adversarial probes for the Python relative-import arm of
//! `resolve_import_path` (lane/py-relative-imports). Each test states the
//! Python semantics it expects; a failing test is a finding.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn targets(r: &ResolutionResult, from: &str) -> Vec<String> {
    let mut v: Vec<String> = r
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Imports && e.source_file == from)
        .map(|e| e.target_file.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

fn imports(r: &ResolutionResult, from: &str, to: &str) -> bool {
    targets(r, from).iter().any(|t| t == to)
}

const EMPTY: &str = "X = 1\n";

// ---- 1. recall ----------------------------------------------------------

#[test]
fn src_layout_sibling_resolves() {
    let r = resolve(&[
        ("src/pkg/a.py", "from .b import f\n"),
        ("src/pkg/b.py", "def f():\n    pass\n"),
    ]);
    assert_eq!(targets(&r, "src/pkg/a.py"), vec!["src/pkg/b.py"]);
}

#[test]
fn src_layout_parent_climb_resolves() {
    let r = resolve(&[
        ("src/pkg/sub/c.py", "from ..b import f\n"),
        ("src/pkg/b.py", "def f():\n    pass\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/sub/__init__.py", ""),
    ]);
    assert!(
        imports(&r, "src/pkg/sub/c.py", "src/pkg/b.py"),
        "{:?}",
        targets(&r, "src/pkg/sub/c.py")
    );
}

#[test]
fn package_dir_mapped_root_relative_import_resolves() {
    // package-dir = {"pkg" = "impl"}: physical path impl/a.py, import name pkg.a.
    let r = resolve(&[
        ("pyproject.toml", "[tool.setuptools]\npackage-dir = {\"pkg\" = \"impl\"}\n"),
        ("impl/a.py", "from .b import f\n"),
        ("impl/b.py", "def f():\n    pass\n"),
    ]);
    assert!(imports(&r, "impl/a.py", "impl/b.py"));
}

#[test]
fn relative_import_of_sibling_module_via_dot_name() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/a.py", "from . import b\n"),
        ("pkg/b.py", EMPTY),
    ]);
    assert!(
        imports(&r, "pkg/a.py", "pkg/b.py") || imports(&r, "pkg/a.py", "pkg/__init__.py"),
        "{:?}",
        targets(&r, "pkg/a.py")
    );
}

// ---- 2. stubs and friends ----------------------------------------------

#[test]
fn pyi_only_sibling_characterized() {
    let r = resolve(&[("pkg/a.py", "from .b import f\n"), ("pkg/b.pyi", "def f() -> None: ...\n")]);
    eprintln!("PYI targets: {:?}", targets(&r, "pkg/a.py"));
}

#[test]
fn pyw_sibling_characterized() {
    let r = resolve(&[("pkg/a.py", "from .b import f\n"), ("pkg/b.pyw", "def f():\n    pass\n")]);
    eprintln!("PYW targets: {:?}", targets(&r, "pkg/a.py"));
}

#[test]
fn pyi_importer_characterized() {
    let r = resolve(&[("pkg/a.pyi", "from .b import f\n"), ("pkg/b.py", "def f():\n    pass\n")]);
    eprintln!("PYI importer targets: {:?}", targets(&r, "pkg/a.pyi"));
}

// ---- 3. namespace packages ---------------------------------------------

#[test]
fn ns_pkg_x_init() {
    let r = resolve(&[("ns/a.py", "from .x import y\n"), ("ns/x/__init__.py", "y = 1\n")]);
    assert!(imports(&r, "ns/a.py", "ns/x/__init__.py"));
}

#[test]
fn ns_pkg_x_py() {
    let r = resolve(&[("ns/a.py", "from .x import y\n"), ("ns/x.py", "y = 1\n")]);
    assert!(imports(&r, "ns/a.py", "ns/x.py"));
}

#[test]
fn ns_pkg_x_is_namespace_dir_dotted_module() {
    let r = resolve(&[("ns/a.py", "from .x.m import y\n"), ("ns/x/m.py", "y = 1\n")]);
    assert!(imports(&r, "ns/a.py", "ns/x/m.py"));
}

#[test]
#[ignore = "pre-existing recall gap: submodule through a namespace dir; follow-up on ft-bd1bfac9"]
fn ns_pkg_x_is_namespace_dir_from_import_submodule() {
    // `from .x import m` where x/ has no __init__: m is the submodule ns/x/m.py.
    let r = resolve(&[("ns/a.py", "from .x import m\n"), ("ns/x/m.py", "y = 1\n")]);
    assert!(imports(&r, "ns/a.py", "ns/x/m.py"), "{:?}", targets(&r, "ns/a.py"));
}

#[test]
fn ns_miss_never_becomes_an_edge_to_an_unrelated_file() {
    let r = resolve(&[
        ("ns/a.py", "from .x import y\n"),
        ("ns/x/m.py", "y = 1\n"),
        ("src/ns/x.py", "y = 2\n"),
        ("ns/ns/x.py", "y = 3\n"),
    ]);
    assert!(targets(&r, "ns/a.py").is_empty(), "{:?}", targets(&r, "ns/a.py"));
}

// ---- 4. climb ------------------------------------------------------------

#[test]
fn dot_only_never_binds_a_module_file_beside_the_package_dir() {
    // `from . import x` in pkg/a.py names package `pkg`; `pkg.py` is not it.
    let r = resolve(&[("pkg/a.py", "from . import x\n"), ("pkg.py", EMPTY)]);
    assert!(!imports(&r, "pkg/a.py", "pkg.py"), "{:?}", targets(&r, "pkg/a.py"));
}

#[test]
fn dot_only_with_init_present_never_adds_the_beside_file() {
    let r = resolve(&[
        ("pkg/a.py", "from . import x\n"),
        ("pkg/__init__.py", EMPTY),
        ("pkg.py", EMPTY),
    ]);
    let t = targets(&r, "pkg/a.py");
    assert!(!t.contains(&"pkg.py".to_string()), "{t:?}");
}

#[test]
fn double_dot_never_binds_parent_dir_as_py_file() {
    let r = resolve(&[("top/pkg/a.py", "from .. import z\n"), ("top.py", EMPTY)]);
    assert!(!imports(&r, "top/pkg/a.py", "top.py"), "{:?}", targets(&r, "top/pkg/a.py"));
}

#[test]
fn climb_to_root_probes_root_init_characterized() {
    let r = resolve(&[("pkg/a.py", "from .. import x\n"), ("__init__.py", EMPTY)]);
    eprintln!("ROOT climb targets: {:?}", targets(&r, "pkg/a.py"));
}

#[test]
fn climb_to_root_dotted_module_resolves() {
    let r = resolve(&[("pkg/a.py", "from ..x import f\n"), ("x.py", "def f():\n    pass\n")]);
    assert!(imports(&r, "pkg/a.py", "x.py"), "{:?}", targets(&r, "pkg/a.py"));
}

#[test]
fn root_file_dot_import_without_root_init_characterized() {
    let r = resolve(&[("main.py", "from . import x\n"), ("x.py", EMPTY)]);
    eprintln!("root file targets: {:?}", targets(&r, "main.py"));
}

#[test]
fn root_file_dot_module_import() {
    let r = resolve(&[("main.py", "from .x import f\n"), ("x.py", "def f():\n    pass\n")]);
    assert!(imports(&r, "main.py", "x.py"));
}

#[test]
fn deep_climb_past_root_is_a_miss() {
    let r = resolve(&[
        ("a/b/c.py", "from .... import q\nfrom ....q import r\n"),
        ("q.py", EMPTY),
        ("a/q.py", EMPTY),
    ]);
    assert!(targets(&r, "a/b/c.py").is_empty(), "{:?}", targets(&r, "a/b/c.py"));
}

#[test]
fn deep_climb_exact_depth_resolves() {
    let r = resolve(&[("a/b/c/d.py", "from ...q import r\n"), ("a/q.py", "r = 1\n")]);
    assert!(imports(&r, "a/b/c/d.py", "a/q.py"), "{:?}", targets(&r, "a/b/c/d.py"));
}

// ---- 6. hostile specs ----------------------------------------------------

#[test]
fn hostile_specs_do_not_panic_or_escape() {
    let sources = [
        "from ..... import x\n",
        "from . import x\n",
        "from .é import y\n",
        "from .日本.語 import y\n",
        "from .a/b import y\n",
        "from .a\\b import y\n",
        "from ../x import y\n",
        "import x\nfrom .  import y\n",
        "from .\u{202e}x import y\n",
        "from .a..b import y\n",
    ];
    for src in sources {
        for importer in ["a.py", "pkg/a.py", "p/q/r.py"] {
            let r = resolve(&[
                (importer, src),
                ("x.py", EMPTY),
                ("é.py", EMPTY),
                ("pkg/é.py", EMPTY),
                ("b.py", EMPTY),
                ("a/b.py", EMPTY),
                ("pkg/a/b.py", EMPTY),
            ]);
            for t in targets(&r, importer) {
                assert!(!t.starts_with('/') && !t.contains(".."), "{src:?} -> {t}");
            }
        }
    }
}

#[test]
fn slash_in_spec_does_not_traverse() {
    let r = resolve(&[("pkg/a.py", "from .x/y import q\n"), ("pkg/x/y.py", EMPTY)]);
    assert!(targets(&r, "pkg/a.py").is_empty(), "{:?}", targets(&r, "pkg/a.py"));
}

// ---- 5. dump for absolute-import edge diffs (ignored; env driven) -------

fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name == "target" || name == ".git" || name.starts_with("target-") || name == "node_modules" || name == "tmp" {
            continue;
        }
        if p.is_dir() {
            walk(base, &p, out);
        } else if name.ends_with(".py") || name.ends_with(".toml") || name.ends_with(".cfg") {
            if let Ok(s) = std::fs::read_to_string(&p) {
                out.push((p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), s));
            }
        }
    }
}

/// Inject relative-import files into the real manifest fixtures (the fixtures
/// themselves contain none) and require them to resolve physically.
#[test]
fn relative_imports_inside_manifest_fixtures_resolve() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy");
    for name in ["python-hatch", "python-setupcfg", "python-ruff"] {
        let base = manifest.join(name);
        let mut files: Vec<(String, String)> = Vec::new();
        walk(&base, &base, &mut files);
        files.push(("python/rootpkg/rel.py".into(), "from .util import split\n".into()));
        files.push(("python/rootpkg/sub/__init__.py".into(), String::new()));
        files.push(("python/rootpkg/sub/deep.py".into(), "from ..util import split\n".into()));
        let borrowed: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let extractions: Vec<Extraction> = borrowed.iter().map(|(p, s)| extract_file(p, s)).collect();
        let mut resolver = Resolver::new();
        resolver.index_project_manifests(&devmap_extract::collect_project_manifests(&base).unwrap());
        resolver.index_extractions(&extractions);
        let r = resolver.resolve_all(&extractions).unwrap();
        assert!(imports(&r, "python/rootpkg/rel.py", "python/rootpkg/util.py"), "{name}");
        assert!(imports(&r, "python/rootpkg/sub/deep.py", "python/rootpkg/util.py"), "{name}");
    }
}

#[test]
#[ignore]
fn dump_import_edges() {
    let root = std::env::var("DUMP_ROOT").unwrap();
    let out = std::env::var("DUMP_OUT").unwrap();
    let base = std::path::Path::new(&root);
    let mut files: Vec<(String, String)> = Vec::new();
    walk(base, base, &mut files);
    files.sort();
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let extractions: Vec<Extraction> = borrowed.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_project_manifests(&devmap_extract::collect_project_manifests(base).unwrap());
    resolver.index_extractions(&extractions);
    let r = resolver.resolve_all(&extractions).unwrap();
    let mut lines: Vec<String> = r
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Imports)
        .map(|e| format!("{} -> {}", e.source_file, e.target_file))
        .collect();
    lines.sort();
    lines.dedup();
    std::fs::write(out, lines.join("\n")).unwrap();
}
