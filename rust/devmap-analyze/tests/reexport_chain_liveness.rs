//! W1.3, the liveness half — a barrel project's public API is not dead.
//!
//! Lives here rather than beside the other re-export tests because it needs
//! `devmap-analyze`, and `devmap-resolve` must not depend on the crate that
//! consumes it.

use devmap_extract::extract_file;
use devmap_resolve::Resolver;

/// A barrel-file project does not report its own public API as dead.
///
/// The work order's stated acceptance. It passed before this change too — the
/// global lookup found the symbol — and is kept because it is the property a
/// reader cares about, and because the disambiguation above must not break it.
#[test]
fn a_barrel_project_does_not_report_its_public_api_as_dead() {
    let files: &[(&str, &str)] = &[
        (
            "impl.ts",
            "export function thing(): number {\n  return 1;\n}\n",
        ),
        ("index.ts", "export { thing } from './impl';\n"),
        ("app.ts", "import { thing } from './index';\n\nthing();\n"),
    ];
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let reports = devmap_analyze::analyze_liveness(&extractions, &resolution);
    let dead: Vec<_> = reports
        .iter()
        .filter(|r| !r.is_exempt && r.symbol_name.contains("thing"))
        .collect();
    assert!(
        dead.is_empty(),
        "the barrel's own export is reached: {dead:?}"
    );
}
