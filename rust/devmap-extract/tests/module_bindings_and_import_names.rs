//! Module-level bindings are symbols, and import lists bind the names they name.
//!
//! Reported as "DevMap doesn't index `REQUIRED_CONTROLS`". The constant is a
//! module-level `REQUIRED_CONTROLS: tuple[str, ...] = (...)` in a module whose
//! `__all__` does not list it, and a test imports it by name. The Python
//! extractor kept a module binding only when `__all__` named it, so the
//! constant was not in the graph: unsearchable, no target for its importer, no
//! blast radius. Auditing the class turned up the same loss in other shapes and
//! languages, and two import readers that fabricated names out of text:
//!
//! - Python bindings under a module-level `try`/`if`/`with`/`for` were treated as
//!   block-scoped, so they were dropped even when `__all__` listed them.
//! - Python unpacking (`A, B = …`) bound nothing.
//! - `from m import (  # noqa\n  NAME,  # why\n)` bound `#` and lost `NAME`;
//!   `import a.b as c, d` aliased `a.b` to `c, d`; `import os, sys` was one
//!   module named `os, sys`.
//! - TypeScript `import { type Foo, bar }` bound `type` and lost `Foo`, and a
//!   comment in a clause bound `//`.
//! - TypeScript destructuring emitted one symbol named `{ a, b }`; a declarator
//!   with no initializer was dropped; `const local = 1; export { local };`
//!   dropped `local`.
//! - Go `const P, Q = 1, 2` kept only `P`.
//! - Type aliases (`pub type R = …`, Go `type A = B`, Python `type V = …`) were
//!   never symbols.
//!
//! Every case below failed on the extractor before the fix.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, SymbolKind};

fn named(extraction: &Extraction, kind: SymbolKind) -> Vec<(String, bool)> {
    extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == kind)
        .map(|symbol| (symbol.name.clone(), symbol.is_exported))
        .collect()
}

fn names(extraction: &Extraction, kind: SymbolKind) -> Vec<String> {
    named(extraction, kind)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// A binding only ever names an identifier. Any symbol, import or export name
/// carrying punctuation or whitespace was read out of text rather than out of
/// the tree, and names nothing a program can reference.
fn assert_no_fabricated_names(extraction: &Extraction) {
    let fabricated = |name: &str| {
        name.is_empty()
            || name.chars().any(|c| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '#' | '/' | '{' | '}' | '[' | ']' | ',' | '(' | ')' | '*' | '='
                    )
            })
    };
    for symbol in &extraction.symbols {
        if symbol.kind == SymbolKind::File {
            continue;
        }
        assert!(
            !fabricated(&symbol.name),
            "fabricated symbol name {:?} in {}",
            symbol.name,
            extraction.file_path
        );
    }
    for import in &extraction.imports {
        for name in import.imported_names.iter().chain(&import.local_names) {
            assert!(
                !fabricated(name),
                "fabricated import name {name:?} in {:?}",
                import.raw_import
            );
        }
        if let Some(alias) = &import.alias {
            assert!(
                !fabricated(alias),
                "fabricated import alias {alias:?} in {:?}",
                import.raw_import
            );
        }
    }
    for export in &extraction.exports {
        if export.exported_name != "*" {
            assert!(
                !fabricated(&export.exported_name),
                "fabricated export {:?}",
                export.exported_name
            );
        }
    }
}

// ---------------------------------------------------------------- Python ----

/// The reported shape, verbatim in structure: an annotated tuple constant a
/// module's `__all__` omits, which another module imports by name.
///
/// `__all__` governs `from m import *` and nothing else; every module-scope
/// name is importable. `is_exported` still follows `__all__`, as it does for a
/// function, so liveness judges the constant by its edges.
#[test]
fn a_python_constant_is_a_symbol_whether_or_not_all_names_it() {
    let extraction = extract_file(
        "python/qd_train/ledger.py",
        "__all__ = [\"NOT_APPLICABLE\"]\n\
         NOT_APPLICABLE = \"n/a:build\"\n\
         REQUIRED_GATES: tuple[str, ...] = (\n    \"ece\",\n)\n\
         REQUIRED_CONTROLS: tuple[str, ...] = (\n    \"shuffled_label\",\n    \"privileged_hunk\",\n    \"degenerate_head\",\n    \"transfer_gate\",\n)\n\
         _PRIVATE = 1\n\
         __version__ = \"1.0\"\n\
         class Ledger:\n    ATTR = 5\n    def run(self):\n        LOCAL = 6\n\
         def helper():\n    INNER = 7\n    return INNER\n",
    );
    assert_eq!(
        named(&extraction, SymbolKind::Variable),
        [
            ("NOT_APPLICABLE".to_string(), true),
            ("REQUIRED_GATES".to_string(), false),
            ("REQUIRED_CONTROLS".to_string(), false),
            ("_PRIVATE".to_string(), false),
            ("__version__".to_string(), false),
        ],
        "every module-scope binding is a symbol; `ATTR` is a class attribute and \
         `LOCAL`/`INNER` are function locals"
    );
    let controls = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "REQUIRED_CONTROLS")
        .expect("REQUIRED_CONTROLS is a symbol");
    assert_eq!(
        controls.qualified_name,
        "python/qd_train/ledger.py::REQUIRED_CONTROLS"
    );
    assert_eq!(
        controls.parent_symbol.as_deref(),
        Some("python/qd_train/ledger.py")
    );
    assert_no_fabricated_names(&extraction);
}

/// A module with no `__all__` still declares its constants.
#[test]
fn a_module_without_all_still_declares_its_constants() {
    let extraction = extract_file("cfg.py", "TIMEOUT = 30\nRETRIES: int = 3\n");
    assert_eq!(
        names(&extraction, SymbolKind::Variable),
        ["TIMEOUT", "RETRIES"]
    );
}

/// Python scopes are functions, lambdas and classes — never blocks.
#[test]
fn a_python_binding_under_a_module_level_block_is_module_scope() {
    let extraction = extract_file(
        "opt.py",
        "try:\n    import numpy\n    HAS_NUMPY = True\nexcept ImportError:\n    HAS_NUMPY = False\n\
         if TYPE_CHECKING:\n    PathLike = str\nelse:\n    PathLike = bytes\n\
         with open('x') as fh:\n    DATA = fh.read()\n\
         for _i in range(3):\n    LAST = _i\n\
         while False:\n    NEVER = 1\n\
         match MODE:\n    case 1:\n        MATCHED = 1\n\
         def f():\n    if True:\n        IN_FUNCTION = 1\n\
         class K:\n    if True:\n        IN_CLASS = 1\n",
    );
    assert_eq!(
        names(&extraction, SymbolKind::Variable),
        ["HAS_NUMPY", "PathLike", "DATA", "LAST", "NEVER", "MATCHED"],
        "one symbol per module attribute; a block under a function or class is \
         still that function's or class's scope"
    );
}

/// Unpacking binds each name it names, and nothing it writes into.
#[test]
fn python_unpacking_binds_every_name() {
    let extraction = extract_file(
        "unpack.py",
        "A, B = 1, 2\n(C, D) = 3, 4\n[E, F] = 5, 6\nG, *H = [1, 2, 3]\n\
         (I, (J, K)) = 1, (2, 3)\nobj.attr, L = 1, 2\nM[0], N = 1, 2\nX = Y = 0\n",
    );
    assert_eq!(
        names(&extraction, SymbolKind::Variable),
        ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "N", "X", "Y"],
        "`obj.attr` and `M[0]` write into existing objects and bind no name"
    );
    assert_no_fabricated_names(&extraction);
}

/// `f = wrap(f)` rebinds the attribute `def f` declared; it is one symbol.
#[test]
fn a_rebound_function_is_one_symbol() {
    let extraction = extract_file("w.py", "def f():\n    return 1\n\nf = wrap(f)\n");
    let f: Vec<SymbolKind> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "w.py::f")
        .map(|symbol| symbol.kind)
        .collect();
    assert_eq!(f, [SymbolKind::Function]);
}

/// Names read, per the extractor's `Name` references, and where.
fn name_reads(extraction: &Extraction, name: &str) -> Vec<Option<String>> {
    extraction
        .references
        .iter()
        .filter(|reference| reference.name == name)
        .filter(|reference| reference.kind == devmap_extract::model::ReferenceKind::Name)
        .map(|reference| reference.enclosing_symbol.clone())
        .collect()
}

/// A module binding is not a local of the module, so a read of it at module
/// level is a reference — the shape that left script constants with no edge.
/// A function's own binding of the same name still shadows it there.
#[test]
fn a_module_level_read_of_a_module_binding_is_a_reference() {
    let extraction = extract_file(
        "scripts/summarize.py",
        "from pathlib import Path\n\
         OUT = Path(__file__).resolve().parent\n\
         DERIVED = OUT / \"x\"\n\
         print(OUT)\n\
         def reads_module():\n    return OUT\n\
         def shadows():\n    OUT = 2\n    return OUT\n",
    );
    assert_eq!(
        name_reads(&extraction, "OUT"),
        [
            None,
            None,
            Some("scripts/summarize.py::reads_module".to_string()),
        ],
        "two module-level reads and one function read; `shadows` binds its own `OUT`"
    );

    // An augmented assignment rebinds the same module attribute; it does not
    // turn the name into a module local that hides the reads.
    let extended = extract_file(
        "scripts/report.py",
        "lines = ['# Report']\nlines += ['', 'body']\nprint('\\n'.join(lines))\n",
    );
    assert_eq!(name_reads(&extended, "lines"), [None]);
}

/// The same holds for an exported constant in the brace languages.
#[test]
fn a_module_level_read_of_an_exported_constant_is_a_reference() {
    let go = extract_file(
        "c.go",
        "package p\nconst Limit = 10\nvar Twice = Limit * 2\nconst lower = 1\nvar alsoLower = lower + 1\n",
    );
    assert_eq!(
        name_reads(&go, "Limit").len(),
        1,
        "exported `Limit` is read once"
    );
    assert!(
        name_reads(&go, "lower").is_empty(),
        "an unexported Go constant is no symbol, so it stays a package local"
    );

    let rust = extract_file(
        "c.rs",
        "pub const LIMIT: u32 = 10;\npub static TWICE: u32 = LIMIT * 2;\n",
    );
    assert_eq!(name_reads(&rust, "LIMIT").len(), 1);

    let ts = extract_file(
        "c.ts",
        "export const LIMIT = 10;\nexport const TWICE = LIMIT * 2;\n",
    );
    assert_eq!(name_reads(&ts, "LIMIT").len(), 1);
}

/// A call's argument label is neither a binding nor a read.
///
/// Read as a binding, `dict(codegraph="CodeGraph")` filed `codegraph` as a
/// module local and hid the later module-level reads of the real constant
/// `codegraph`, which was then reported dead at 0.9; inside a function,
/// `run(config=config)` lost its read of the module's `config`. Each case
/// below has exactly one real read — the value — so a label that shadowed it
/// shows up as zero and a label emitted as a read shows up as two.
#[test]
fn an_argument_label_is_neither_a_binding_nor_a_read() {
    let python = extract_file(
        "labels.py",
        "NAMES = dict(codegraph=\"CodeGraph\")\n\
         codegraph = read()\n\
         found = search(codegraph)\n\
         config = load()\n\
         def main():\n    return run(config=config)\n",
    );
    assert_eq!(
        name_reads(&python, "codegraph"),
        [None],
        "one module-level read; the `codegraph=` label is neither"
    );
    assert_eq!(
        name_reads(&python, "config"),
        [Some("labels.py::main".to_string())],
        "the value `config` is read inside `main`; the label is not"
    );

    for (path, source, name) in [
        (
            "Labels.cs",
            "class K { void M() { F(handler: handler); } }\n",
            "handler",
        ),
        ("labels.r", "g <- function() h(limit = limit)\n", "limit"),
        (
            "Labels.sol",
            "contract C { function f() public { g({amount: amount}); } }\n",
            "amount",
        ),
    ] {
        let extraction = extract_file(path, source);
        assert_eq!(
            name_reads(&extraction, name).len(),
            1,
            "{path}: one read of `{name}`, the value: {:?}",
            extraction
                .references
                .iter()
                .map(|reference| (&reference.name, reference.kind))
                .collect::<Vec<_>>()
        );
    }
}

/// PEP 695 type aliases are named types.
#[test]
fn a_python_type_alias_is_a_type() {
    let extraction = extract_file(
        "types.py",
        "type Vec = list[float]\ntype Pair[T] = tuple[T, T]\ndef f():\n    type Local = int\n",
    );
    assert_eq!(names(&extraction, SymbolKind::Interface), ["Vec", "Pair"]);
}

/// The import shape that hid three of seven importers of a real constant: a
/// lint pragma on the opening line of a parenthesized import list.
#[test]
fn python_import_names_survive_comments() {
    let extraction = extract_file(
        "tools/run.py",
        "from qd_train.ledger import (  # noqa: E402\n\
         \x20   DEFAULT_LEDGER_PATH,  # why it is here\n\
         \x20   # a standalone comment\n\
         \x20   Ledger as L,\n\
         \x20   RunRecorder,\n\
         )\n",
    );
    let import = extraction
        .imports
        .iter()
        .find(|import| import.module_specifier == "qd_train.ledger")
        .expect("the from-import is recorded");
    assert_eq!(
        import.imported_names,
        ["DEFAULT_LEDGER_PATH", "Ledger", "RunRecorder"]
    );
    assert_eq!(
        import.local_names,
        ["DEFAULT_LEDGER_PATH", "L", "RunRecorder"]
    );
    assert_no_fabricated_names(&extraction);
}

/// `import a, b` binds two modules, each with its own alias.
#[test]
fn a_python_import_of_several_modules_is_several_imports() {
    let extraction = extract_file("m.py", "import os, sys\nimport a.b as c, d\nimport e.f\n");
    let imports: Vec<(&str, Option<&str>)> = extraction
        .imports
        .iter()
        .map(|import| (import.module_specifier.as_str(), import.alias.as_deref()))
        .collect();
    assert_eq!(
        imports,
        [
            ("os", None),
            ("sys", None),
            ("a.b", Some("c")),
            ("d", None),
            ("e.f", None)
        ]
    );
    assert_no_fabricated_names(&extraction);
}

/// Relative modules keep their dots; a wildcard binds no name.
///
/// Ported from the text reader's tests: a `*` bound as a name made the
/// resolver bind a local called `*`, so every later reference to a real symbol
/// from that module resolved against a binding that does not exist.
#[test]
fn python_relative_and_wildcard_imports_bind_what_they_name() {
    let extraction = extract_file(
        "pkg/mod.py",
        "from . import sibling\nfrom ..parent import thing as other\nfrom star import *\n",
    );
    let imports: Vec<(&str, Vec<String>, Vec<String>)> = extraction
        .imports
        .iter()
        .map(|import| {
            (
                import.module_specifier.as_str(),
                import.imported_names.clone(),
                import.local_names.clone(),
            )
        })
        .collect();
    assert_eq!(
        imports,
        [
            (
                ".",
                vec!["sibling".to_string()],
                vec!["sibling".to_string()]
            ),
            (
                "..parent",
                vec!["thing".to_string()],
                vec!["other".to_string()]
            ),
            ("star", vec![], vec![]),
        ]
    );
}

// -------------------------------------------------------------------- Go ----

/// `name` is a repeated field; every name in a spec is declared.
#[test]
fn go_multi_name_specs_declare_every_name() {
    let extraction = extract_file(
        "p.go",
        "package p\nconst P, Q = 1, 2\nvar (\n\tX, Y = 1, 2\n\tlower, Upper = 3, 4\n)\nconst (\n\tA = iota\n\tB\n)\n",
    );
    assert_eq!(
        names(&extraction, SymbolKind::Variable),
        ["P", "Q", "X", "Y", "Upper", "A", "B"],
        "Go exports by capitalisation, per name; `lower` stays out"
    );
    assert_no_fabricated_names(&extraction);
}

/// The grammar spells an alias `type_alias`, not `type_spec`.
#[test]
fn a_go_type_alias_is_a_type() {
    let extraction = extract_file(
        "t.go",
        "package p\ntype A = B\ntype (\n\tD = int\n\tE struct{}\n)\ntype C B\n",
    );
    let mut types = names(&extraction, SymbolKind::Struct);
    types.sort();
    assert_eq!(types, ["A", "C", "D", "E"]);
}

// ------------------------------------------------------------------ Rust ----

/// Module-level aliases are types; an associated type in an `impl` is not a
/// module symbol.
#[test]
fn a_rust_type_alias_is_a_type() {
    let extraction = extract_file(
        "t.rs",
        "pub type Res<T> = Result<T, ()>;\ntype Priv = u8;\nmod inner { pub type Inner = u16; }\n\
         struct S;\nimpl Iterator for S { type Item = u8; fn next(&mut self) -> Option<u8> { None } }\n\
         fn f() { type Local = u32; }\n",
    );
    assert_eq!(
        named(&extraction, SymbolKind::Interface),
        [
            ("Res".to_string(), true),
            ("Priv".to_string(), false),
            ("Inner".to_string(), true),
        ]
    );
}

// --------------------------------------------------------------- TS / JS ----

/// A destructuring declarator binds each name in its pattern and fabricates
/// nothing.
#[test]
fn a_destructuring_declarator_binds_each_name() {
    let extraction = extract_file(
        "d.ts",
        "export const { a, b: renamed, c = 1, ...rest } = obj;\n\
         export const [x, , y = 2, ...zs] = arr;\n\
         export const { nested: { deep } } = obj;\n",
    );
    assert_eq!(
        names(&extraction, SymbolKind::Variable),
        ["a", "renamed", "c", "rest", "x", "y", "zs", "deep"]
    );
    assert_no_fabricated_names(&extraction);
}

/// A declarator with no initializer still binds its name.
#[test]
fn a_declarator_without_an_initializer_is_a_binding() {
    let extraction = extract_file("u.ts", "export let pending: number;\nexport var later;\n");
    assert_eq!(
        names(&extraction, SymbolKind::Variable),
        ["pending", "later"]
    );
}

/// `export { local }`, `export { local as Public }` and `export default local`
/// publish a local binding as surely as `export const` does.
#[test]
fn clause_and_default_exports_publish_local_bindings() {
    let extraction = extract_file(
        "e.ts",
        "const local = 1;\nconst renamed = 2;\nconst dflt = 3;\nconst hidden = 4;\n\
         function helper() { return 1; }\n\
         export { local, helper };\nexport { renamed as Public };\nexport default dflt;\n",
    );
    assert_eq!(
        named(&extraction, SymbolKind::Variable),
        [
            ("local".to_string(), true),
            ("renamed".to_string(), true),
            ("dflt".to_string(), true),
        ],
        "`hidden` is exported by nothing and stays out, as a private Rust or Go \
         constant does"
    );
    let helper = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "helper")
        .expect("helper is a function symbol");
    assert!(
        helper.is_exported,
        "a clause export publishes a function too"
    );

    let exported: Vec<(&str, Option<&str>)> = extraction
        .exports
        .iter()
        .map(|export| (export.exported_name.as_str(), export.local_name.as_deref()))
        .collect();
    assert!(
        exported.contains(&("Public", Some("renamed"))),
        "the renamed export keeps its public name: {exported:?}"
    );
    assert!(
        !exported.iter().any(|(name, _)| *name == "renamed"),
        "`renamed` is exported as `Public`, never as itself: {exported:?}"
    );
    assert_no_fabricated_names(&extraction);
}

/// Comments and inline `type` modifiers are not names.
#[test]
fn ts_import_names_survive_comments_and_type_modifiers() {
    let extraction = extract_file(
        "i.ts",
        "import { type Foo, bar, // why\n  baz /* inline */, type Qux as Q } from './x';\n\
         export { type Foo as PublicFoo, // re-exported\n  bar } from './x';\n",
    );
    let import = extraction
        .imports
        .iter()
        .find(|import| import.module_specifier == "./x" && import.raw_import.starts_with("import"))
        .expect("the named import is recorded");
    assert_eq!(import.imported_names, ["Foo", "bar", "baz", "Qux"]);
    assert_eq!(import.local_names, ["Foo", "bar", "baz", "Q"]);
    // The `File` node is itself `is_exported` and so publishes an export named
    // after the file; that record is not under test here.
    let exported: Vec<&str> = extraction
        .exports
        .iter()
        .map(|export| export.exported_name.as_str())
        .filter(|name| *name != "i.ts")
        .collect();
    assert_eq!(exported, ["PublicFoo", "bar"]);
    assert_no_fabricated_names(&extraction);
}

/// The same reader serves JavaScript, which has no `type` modifier but does
/// have comments.
#[test]
fn js_import_names_survive_comments() {
    let extraction = extract_file("i.js", "import { a, // c\n  b } from './y';\n");
    let import = extraction.imports.first().expect("the import is recorded");
    assert_eq!(import.imported_names, ["a", "b"]);
    assert_no_fabricated_names(&extraction);
}
