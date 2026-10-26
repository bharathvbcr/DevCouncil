//! Round-four adversarial probes for the Python extractor changes in 042133bf
//! (decorator classifier, dotted-segment reader, torch dispatch gate,
//! module-scope rebinding marks). A failing test is a BREAK of the new code.

use devmap_extract::extract_file;
use devmap_extract::model::{ExtractedReference, ReferenceKind, WiringKind};

fn field_recorded(source: &str) -> bool {
    extract_file("h.py", source)
        .references
        .iter()
        .any(|r: &ExtractedReference| r.assigned_to.as_deref() == Some("self.x"))
}

fn static_method(decorator: &str, prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}class H:\n    {decorator}\n    def make(self):\n        self.x = A()\n{suffix}"
    )
}

// ------------------------------------------------------------ 3. decorators

#[test]
fn from_builtins_import_staticmethod_is_the_builtin() {
    // `from builtins import staticmethod` binds the builtin itself. Regression
    // vs 103272ea: the base reads a bare `staticmethod` as the builtin.
    let src = static_method("@staticmethod", "from builtins import staticmethod\n", "");
    assert!(!field_recorded(&src), "a non-instance method wrote a field");
}

#[test]
fn dunder_import_builtins_attribute_is_the_builtin() {
    for decorator in [
        "@__import__('builtins').staticmethod",
        "@__import__(\"builtins\").classmethod",
        "@__import__( 'builtins' ).classmethod",
    ] {
        assert!(
            !field_recorded(&static_method(decorator, "", "")),
            "{decorator}"
        );
    }
}

#[test]
fn builtins_name_rebound_locally_is_not_the_builtins_module() {
    let src = static_method(
        "@builtins.staticmethod",
        "import builtins\nclass Fake:\n    staticmethod = None\nbuiltins = Fake()\n",
        "",
    );
    assert!(
        field_recorded(&src),
        "`builtins` was rebound to a Fake: instance method"
    );
}

#[test]
fn builtins_alias_rebound_is_not_the_builtins_module() {
    let src = static_method(
        "@b.staticmethod",
        "import builtins as b\nimport functools\nb = functools\n",
        "",
    );
    assert!(
        field_recorded(&src),
        "`b` was rebound to functools: instance method"
    );
}

#[test]
fn staticmethod_rebound_only_after_the_class_is_still_the_builtin() {
    // The class body runs before the later `def staticmethod`: builtin.
    let src = static_method(
        "@staticmethod",
        "",
        "\ndef staticmethod(f):\n    return f\n",
    );
    assert!(
        !field_recorded(&src),
        "order matters: the decorator ran with the builtin"
    );
}

#[test]
fn staticmethod_rebound_in_another_scope_only_is_the_builtin() {
    for suffix in [
        "\ndef f():\n    staticmethod = 1\n    return staticmethod\n",
        "\ndef g(staticmethod):\n    return staticmethod\n",
        "\ndef k():\n    for staticmethod in range(3):\n        pass\n",
        "\nclass Other:\n    staticmethod = 1\n",
        "\ndef w():\n    import deco as staticmethod\n    return staticmethod\n",
        "\ndef w():\n    from deco import staticmethod\n",
    ] {
        let src = static_method("@staticmethod", "", suffix);
        assert!(
            !field_recorded(&src),
            "{suffix:?}: another scope's name changed the module's"
        );
    }
}

#[test]
fn global_declared_but_never_assigned_does_not_unbind_the_builtin() {
    let src = static_method(
        "@staticmethod",
        "",
        "\ndef f():\n    global staticmethod\n    return staticmethod\n",
    );
    assert!(!field_recorded(&src), "`global` alone binds nothing");
}

#[test]
fn many_static_methods_in_one_file_extract_in_bounded_time() {
    // Each decorator reads the whole file's rebinding set again.
    let mut src = String::from("class H:\n");
    for i in 0..3000 {
        src.push_str(&format!(
            "    @staticmethod\n    def m{i}(self):\n        self.x = A()\n"
        ));
    }
    let start = std::time::Instant::now();
    let ex = extract_file("h.py", &src);
    let took = start.elapsed();
    assert!(!ex.symbols.is_empty());
    assert!(
        took.as_secs_f64() < 5.0,
        "3000 decorators took {took:?} (quadratic file walks)"
    );
}

// -------------------------------------------------------- 4. dotted segments

fn heritage_names(source: &str) -> Vec<String> {
    extract_file("w.py", source)
        .references
        .iter()
        .filter(|r| {
            matches!(
                r.kind,
                ReferenceKind::Heritage | ReferenceKind::HeritageInterface
            )
        })
        .map(|r| r.name.clone())
        .collect()
}

const OPAQUE: &str = devmap_extract::model::OPAQUE_BASE;

#[test]
fn spaces_around_dots_are_the_same_dotted_base() {
    assert_eq!(
        heritage_names("class X(a .b . C):\n    pass\n"),
        ["C", "a", "b"]
    );
    assert_eq!(
        heritage_names("class X(a. b .C):\n    pass\n"),
        ["C", "a", "b"]
    );
}

#[test]
fn indexed_or_called_qualifier_is_opaque_not_a_dotted_base() {
    for head in [
        "a.b[0].C",
        "getattr(a, 'b').C",
        "a().b.C",
        "a.b().C",
        "(a or b).C",
        "a[0].C",
    ] {
        assert_eq!(
            heritage_names(&format!("class X({head}):\n    pass\n")),
            [OPAQUE],
            "{head}"
        );
    }
}

#[test]
fn parenthesised_dotted_base_is_the_dotted_base() {
    assert_eq!(
        heritage_names("class X((a.b.C)):\n    pass\n"),
        ["C", "a", "b"]
    );
    assert_eq!(
        heritage_names("class X((a).b.C):\n    pass\n"),
        ["C", "a", "b"]
    );
}

#[test]
fn unicode_identifiers_are_dotted_segments() {
    assert_eq!(
        heritage_names("class X(módulo.Basé):\n    pass\n"),
        ["Basé", "módulo"]
    );
    assert_eq!(
        heritage_names("class X(日本.基底.クラス):\n    pass\n"),
        ["クラス", "日本", "基底"]
    );
}

#[test]
fn a_500_segment_dotted_base_keeps_every_segment() {
    let chain: Vec<String> = (1..=500).map(|i| format!("a{i}")).collect();
    let src = format!("class X({}.C):\n    pass\n", chain.join("."));
    let got = heritage_names(&src);
    assert_eq!(got.len(), 501);
    assert_eq!(got[0], "C");
    assert_eq!(got[1], "a1");
    assert_eq!(got[500], "a500");
}

#[test]
fn a_parameterised_base_with_a_numeric_type_argument_is_still_a_named_base() {
    // `Array[float, 3]` is a generic parameterisation, not an index into a
    // runtime value.
    assert_eq!(
        heritage_names("class X(Array[float, 3]):\n    pass\n"),
        ["Array"]
    );
    assert_eq!(
        heritage_names("class X(Annotated[int, 3]):\n    pass\n"),
        ["Annotated"]
    );
}

#[test]
fn index_expressions_are_opaque() {
    for head in [
        "bases[0]",
        "bases[-1]",
        "bases[1:]",
        "bases[i + 1]",
        "bases[1.5]",
        "bases[i][0]",
    ] {
        let got = heritage_names(&format!("class X({head}):\n    pass\n"));
        assert_eq!(got, [OPAQUE], "{head}");
    }
}

// ---------------------------------------------------------- 8. torch dispatch

fn dispatched(source: &str) -> bool {
    extract_file("m.py", source)
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::RuntimeEntryPoint && w.target_symbol.contains("forward"))
}

#[test]
fn torch_aliased_module_keeps_the_autograd_contract() {
    assert!(dispatched(
        "import torch as T\nclass F(T.autograd.Function):\n    def forward(ctx, x):\n        return x\n"
    ));
}

#[test]
fn from_torch_import_autograd_keeps_the_contract() {
    assert!(dispatched(
        "from torch import autograd\nclass F(autograd.Function):\n    def forward(ctx, x):\n        return x\n"
    ));
}

#[test]
fn from_torch_import_nn_as_alias_keeps_the_module_contract() {
    assert!(dispatched(
        "from torch import nn as N\nclass M(N.Module):\n    def forward(self, x):\n        return x\n"
    ));
}

#[test]
fn bare_function_import_from_torch_autograd_keeps_the_contract() {
    // `from torch.autograd import Function; class F(Function)`: the same class
    // as `autograd.Function`. Pre-existing gap: the registry wants a qualifier.
    assert!(dispatched(
        "from torch.autograd import Function\nclass F(Function):\n    def forward(ctx, x):\n        return x\n"
    ));
}

#[test]
fn torch_import_with_assignment_fallback_keeps_the_contract() {
    assert!(dispatched(
        "try:\n    import torch.nn as nn\nexcept ImportError:\n    nn = None\nclass M(nn.Module):\n    def forward(self, x):\n        return x\n"
    ));
}

#[test]
fn an_unrelated_function_scope_import_of_the_same_name_does_not_drop_the_contract() {
    assert!(dispatched(
        "import torch.nn as nn\ndef other():\n    import local_pkg.nn as nn\n    return nn\nclass M(nn.Module):\n    def forward(self, x):\n        return x\n"
    ));
}

#[test]
fn a_non_torch_package_with_a_torch_prefix_is_not_torch() {
    for import in [
        "import torchvision.nn as nn",
        "import torch_geometric.nn as nn",
        "from torchaudio import nn",
        "from .torch import nn",
        "from torch_ext import nn",
    ] {
        let src =
            format!("{import}\nclass M(nn.Module):\n    def forward(self, x):\n        return x\n");
        assert!(!dispatched(&src), "{import}");
    }
}

#[test]
fn many_torch_methods_in_one_file_extract_in_bounded_time() {
    let mut src = String::from("import torch.nn as nn\n");
    for i in 0..1500 {
        src.push_str(&format!(
            "class M{i}(nn.Module):\n    def forward(self, x):\n        return x\n    def extra_repr(self):\n        return ''\n"
        ));
    }
    let start = std::time::Instant::now();
    let ex = extract_file("m.py", &src);
    let took = start.elapsed();
    assert!(!ex.symbols.is_empty());
    assert!(
        took.as_secs_f64() < 5.0,
        "1500 torch classes took {took:?} (per-method whole-file walks)"
    );
}

// --------------------------------------------------------- 9. hostile input

#[test]
fn ten_thousand_module_scope_del_and_for_statements_extract_in_bounded_time() {
    let mut src = String::from("from a import A\n");
    for i in 0..5000 {
        src.push_str(&format!("v{i} = 1\ndel v{i}\n"));
        src.push_str(&format!("for w{i} in range(1):\n    pass\n"));
    }
    let start = std::time::Instant::now();
    let ex = extract_file("m.py", &src);
    let took = start.elapsed();
    let file_locals = ex.scope_locals.iter().filter(|(s, _)| s == "m.py").count();
    assert!(file_locals >= 10_000, "{file_locals}");
    assert!(took.as_secs_f64() < 5.0, "took {took:?}");
}

#[test]
fn five_thousand_classes_nested_in_functions_extract_in_bounded_time() {
    let mut src = String::new();
    for i in 0..5000 {
        src.push_str(&format!(
            "def build{i}():\n    class H:\n        def go(self):\n            return 1\n    return H\n"
        ));
    }
    let start = std::time::Instant::now();
    let ex = extract_file("m.py", &src);
    let took = start.elapsed();
    assert!(!ex.symbols.is_empty());
    assert!(took.as_secs_f64() < 5.0, "took {took:?}");
}

// ----------------------------------------------- 10. non-Python extraction dump

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// With `DEVMAP_ATTACK4_DUMP_DIR` set, writes the serialized extraction of every
/// non-Python fixture file under `rust/testdata/accuracy` (path-flattened) so
/// two builds can be byte-compared. Unset, it asserts the extraction is
/// deterministic.
#[test]
fn non_python_fixture_extraction_is_deterministic_and_dumpable() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let dump = std::env::var("DEVMAP_ATTACK4_DUMP_DIR").ok();
    let mut count = 0;
    for path in files {
        if path.extension().is_some_and(|e| e == "py" || e == "pyi") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let first = serde_json::to_string(&extract_file(&rel, &text)).unwrap();
        assert_eq!(
            first,
            serde_json::to_string(&extract_file(&rel, &text)).unwrap(),
            "{rel}"
        );
        if let Some(dir) = &dump {
            let flat = rel.replace('/', "__");
            std::fs::write(std::path::Path::new(dir).join(flat), first).unwrap();
        }
        count += 1;
    }
    assert!(count > 50, "only {count} non-Python fixture files found");
}
