//! A bare call never binds to a type alias or an interface in Rust, Python or
//! TypeScript.
//!
//! Type aliases became symbols (`pub type HWND = isize;`). The call ladder's
//! global rung offered every same-named declaration, so `HWND(ptr)` — which
//! constructs the external `windows` crate's tuple struct — bound to the
//! framework's alias at `UniqueGlobal`: thirteen fabricated callers on one
//! corpus. Rust rejects calling an alias (E0423), a Python `type` alias raises
//! when called, and a TypeScript interface or alias has no runtime value, so
//! none of them can be a callee. A tuple struct, which *is* a constructor,
//! keeps its edge, so the fix is a narrowing and not a removal.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn call_targets(resolution: &ResolutionResult, from_file: &str) -> Vec<String> {
    let mut targets: Vec<String> = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_file == from_file)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    targets.sort();
    targets
}

#[test]
fn a_rust_call_does_not_bind_to_a_type_alias() {
    let resolution = resolve(&[
        (
            "platform/windows.rs",
            "pub type HWND = isize;\npub struct Handle(pub isize);\n",
        ),
        (
            "impl/window.rs",
            "use windows::Win32::Foundation::HWND;\n\
             pub fn make() { let _h = HWND(0); let _k = Handle(1); }\n",
        ),
    ]);
    assert_eq!(
        call_targets(&resolution, "impl/window.rs"),
        ["platform/windows.rs::Handle"],
        "the tuple struct is a constructor; the alias never is"
    );
}

#[test]
fn a_python_call_does_not_bind_to_a_type_alias() {
    let resolution = resolve(&[
        (
            "types.py",
            "type Vec = list[float]\n\ndef build():\n    return []\n",
        ),
        ("use.py", "def main():\n    Vec()\n    build()\n"),
    ]);
    assert_eq!(call_targets(&resolution, "use.py"), ["types.py::build"]);
}

#[test]
fn a_typescript_call_does_not_bind_to_an_interface_or_alias() {
    let resolution = resolve(&[
        (
            "shapes.ts",
            "export interface Shape { area(): number }\nexport type Alias = string;\nexport function make(): number { return 1; }\n",
        ),
        ("use.ts", "export function run() { Shape(); Alias(); make(); }\n"),
    ]);
    assert_eq!(call_targets(&resolution, "use.ts"), ["shapes.ts::make"]);
}
