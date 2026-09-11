//! W3.3 — the two rules the Python wiring module held and the kernel did not.
//!
//! `src/devcouncil/indexing/wiring.py` and `devmap-extract/src/wiring.rs`
//! implement overlapping entry-root, test-path and exemption rules in two
//! languages. Duplicated logic, not dead logic — five Python modules still
//! import it — so the two can and will disagree about whether a file is an
//! entry root.
//!
//! The Python side held two things the kernel lacked, and both are real
//! false-positive protection:
//!
//! * **`devcouncil: allow-unwired`** — an explicit author declaration that a
//!   file is intentionally unwired. The kernel ignored it, so it reported files
//!   whose author had already answered the question.
//! * **Dynamic-reference clearing** — a lazily imported plugin, a code-split
//!   route and a worker entry point are all reachable and all invisible to an
//!   import-edge walk.
//!
//! Both directions are asserted throughout: a marker that is always on exempts
//! everything, and a dynamic form that matches everything clears everything.

use devmap_extract::model::WiringKind;
use devmap_extract::wiring::{dynamic_reference_forms, extract_wiring_annotations, ALLOW_UNWIRED};

fn kinds(path: &str, source: &str) -> Vec<WiringKind> {
    extract_wiring_annotations(path, source)
        .into_iter()
        .map(|annotation| annotation.kind)
        .collect()
}

/// The marker must be spelled the same in both implementations.
///
/// A kernel that spells it differently ignores it — silently, and only for the
/// files that use it, which is the hardest kind of divergence to notice.
#[test]
fn the_marker_is_spelled_exactly_as_python_spells_it() {
    let python = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../src/devcouncil/indexing/wiring.py"
    ));
    let Ok(python) = python else {
        // The Rust workspace is vendored on its own in some checkouts.
        eprintln!("skipping parity check: wiring.py not present");
        return;
    };
    let line = python
        .lines()
        .find(|line| line.starts_with("ALLOW_UNWIRED ="))
        .expect("wiring.py defines ALLOW_UNWIRED");
    assert!(
        line.contains(ALLOW_UNWIRED),
        "the kernel spells the marker {ALLOW_UNWIRED:?}, Python spells it {line:?}"
    );
}

#[test]
fn the_marker_produces_an_allow_unwired_annotation() {
    let source =
        format!("# {ALLOW_UNWIRED} — reached by the plugin loader\n\ndef go():\n    pass\n");
    assert!(kinds("pkg/plugin.py", &source).contains(&WiringKind::AllowUnwired));
}

/// OFF: a file without the marker must not be exempted.
#[test]
fn a_file_without_the_marker_gets_no_annotation() {
    let kinds = kinds("pkg/plugin.py", "def go():\n    pass\n");
    assert!(!kinds.contains(&WiringKind::AllowUnwired), "{kinds:?}");
}

/// A near-miss must not count. The marker is an exemption; a typo that still
/// exempted would let the check be switched off by accident.
#[test]
fn a_similar_but_different_marker_does_not_exempt() {
    for near in [
        "# devcouncil: allow_unwired\n",
        "# devcouncil allow-unwired\n",
        "# DEVCOUNCIL: ALLOW-UNWIRED\n",
        "# allow-unwired\n",
    ] {
        let kinds = kinds("pkg/plugin.py", near);
        assert!(
            !kinds.contains(&WiringKind::AllowUnwired),
            "{near:?} was accepted as the marker"
        );
    }
}

// ---------------------------------------------------------------------------
// Dynamic references
// ---------------------------------------------------------------------------

#[test]
fn importlib_names_a_module() {
    let forms = dynamic_reference_forms(
        "pkg/loader.py",
        "import importlib\nm = importlib.import_module('pkg.plugins.alpha')\n",
    );
    assert!(
        forms.contains(&"pkg.plugins.alpha".to_string()),
        "{forms:?}"
    );
    // Also as a path, so it can match `pkg/plugins/alpha.py`.
    assert!(
        forms.contains(&"pkg/plugins/alpha".to_string()),
        "{forms:?}"
    );
}

#[test]
fn a_relative_js_import_resolves_against_the_referring_file() {
    let forms = dynamic_reference_forms("src/routes/index.tsx", "const A = import('./App');\n");
    assert!(forms.contains(&"src/routes/App".to_string()), "{forms:?}");
}

#[test]
fn a_dotted_relative_import_climbs_out_of_its_directory() {
    let forms = dynamic_reference_forms("src/routes/index.tsx", "import('../shared/Panel.tsx')\n");
    assert!(forms.contains(&"src/shared/Panel".to_string()), "{forms:?}");
    assert!(
        forms.contains(&"src/shared/Panel.tsx".to_string()),
        "{forms:?}"
    );
}

#[test]
fn a_worker_url_names_its_entry_point() {
    let forms = dynamic_reference_forms(
        "src/app.ts",
        "const w = new Worker(new URL('./heavy.worker.ts', import.meta.url));\n",
    );
    assert!(forms.contains(&"src/heavy.worker".to_string()), "{forms:?}");
}

#[test]
fn python_dash_m_names_a_module_only_when_the_module_is_quoted() {
    // The pattern requires the module itself to be a string literal, so an
    // unquoted `-m` inside one command string does not match. Measured against
    // `wiring.dynamic_import_keys`, which behaves the same way: this pins the
    // shared limitation rather than a Rust-only one.
    let inline = dynamic_reference_forms("scripts/run.py", "cmd = 'python -m pkg.worker'\n");
    assert!(inline.is_empty(), "{inline:?}");

    let argv = dynamic_reference_forms(
        "scripts/run.py",
        "subprocess.run([sys.executable, \"-m\", \"pkg.worker\"])\n",
    );
    assert!(argv.contains(&"pkg.worker".to_string()), "{argv:?}");
}

/// The port agrees with `wiring.dynamic_import_keys`, form for form.
///
/// Expectations were **measured** from the Python implementation, not written
/// by hand: a table of what the port's author believed Python does would freeze
/// the belief, not the behaviour. Two implementations that disagree about what
/// clears a file are the defect this work order names, so the strongest
/// available assertion is that they produce the same set.
///
/// The sets include entries that name no real file — `src/shared/Panel/tsx`
/// among them. That is deliberate on both sides: this is one half of a set
/// intersection, and a tidier set would clear a different set of files.
#[test]
fn the_port_agrees_with_the_python_implementation() {
    #[allow(clippy::type_complexity)]
    const CASES: &[(&str, &str, &[&str])] = &[
        (
            "pkg/loader.py",
            "m = importlib.import_module('pkg.plugins.alpha')",
            &["pkg.plugins.alpha", "pkg/plugins/alpha"],
        ),
        (
            "pkg/loader.py",
            "m = __import__(\"pkg.late\")",
            &["pkg.late", "pkg/late"],
        ),
        (
            "src/routes/index.tsx",
            "const A = import('./App');",
            &["src.routes.App", "src/routes/App"],
        ),
        (
            "src/App.svelte",
            "const load = () => import('./lib/CloneModal.svelte');",
            &[
                "src.lib.CloneModal",
                "src.lib.CloneModal.svelte",
                "src/lib/CloneModal",
                "src/lib/CloneModal.svelte",
                "src/lib/CloneModal/svelte",
            ],
        ),
        (
            "src/routes/index.tsx",
            "import('../shared/Panel.tsx')",
            &[
                "src.shared.Panel",
                "src.shared.Panel.tsx",
                "src/shared/Panel",
                "src/shared/Panel.tsx",
                "src/shared/Panel/tsx",
            ],
        ),
        (
            "src/app.ts",
            "const w = new Worker(new URL('./heavy.worker.ts', import.meta.url));",
            &[
                "src.heavy.worker",
                "src.heavy.worker.ts",
                "src/heavy.worker",
                "src/heavy.worker.ts",
                "src/heavy/worker",
                "src/heavy/worker/ts",
            ],
        ),
        (
            "scripts/run.py",
            "subprocess.run([sys.executable, \"-m\", \"pkg.worker\"])",
            &["pkg.worker", "pkg/worker"],
        ),
        ("pkg/res.py", "files('pkg.data')", &["pkg.data", "pkg/data"]),
        ("pkg/mod.py", "from pkg import thing", &[]),
        ("pkg/mod.py", "", &[]),
        ("README.md", "import('./App')", &[]),
        (
            "src/a.ts",
            "import('pkg-name/sub')",
            &["pkg-name.sub", "pkg-name/sub"],
        ),
    ];

    for (path, source, expected) in CASES {
        let got = dynamic_reference_forms(path, source);
        let want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            got, want,
            "diverged from wiring.dynamic_import_keys for {path} / {source:?}"
        );
    }
}

/// OFF: a file that references nothing dynamically produces no forms.
///
/// Without this, a pattern that matched everything would clear every file in
/// the corpus and the unwired check would report nothing at all — a filter that
/// silently switches the check off rather than failing.
#[test]
fn ordinary_source_produces_no_dynamic_forms() {
    for source in [
        "from pkg import thing\n\n\ndef main():\n    return thing()\n",
        "import React from 'react';\nexport const A = () => <div/>;\n",
        "# a comment mentioning import( and importlib without a literal\n",
        "",
    ] {
        assert!(
            dynamic_reference_forms("pkg/mod.py", source).is_empty(),
            "produced forms for {source:?}"
        );
    }
}

/// A file the scanner does not read must produce nothing, not a partial answer.
#[test]
fn a_non_code_suffix_is_not_scanned() {
    let source = "import('./App')\n";
    assert!(dynamic_reference_forms("README.md", source).is_empty());
    assert!(dynamic_reference_forms("logo.svg", source).is_empty());
    // …but the config formats that name entry points are scanned, and produce
    // forms when they carry a quoted specifier.
    assert!(!dynamic_reference_forms(
        "pyproject.toml",
        "hook = 'importlib.import_module(\"pkg.build\")'\n"
    )
    .is_empty());
}

#[test]
fn the_annotation_records_the_target_not_the_referrer() {
    let annotations =
        extract_wiring_annotations("src/routes/index.tsx", "const A = import('./App');\n");
    let dynamic: Vec<_> = annotations
        .iter()
        .filter(|a| a.kind == WiringKind::DynamicImport)
        .collect();
    assert!(!dynamic.is_empty());
    for annotation in dynamic {
        assert_ne!(
            annotation.target_symbol, "src/routes/index.tsx",
            "a self-scoped target would make `liveness.rs` read this as a \
             file-level exemption of the referring file, which is the opposite \
             of what it means"
        );
        assert!(annotation.details.contains("src/routes/index.tsx"));
    }
}
