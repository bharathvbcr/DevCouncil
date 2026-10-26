//! Round-three adversarial probes of liveness over Python heritage: the
//! segment reading of a dotted base and the opaque-base marker. A failing test
//! is a BREAK.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;

const OPAQUE_BASE: &str = "<opaque>";

/// Exempt, with a reason that names the contract.
fn names_contract(report: &DeadSymbolReport, word: &str) -> bool {
    report.is_exempt
        && report
            .exemption_reason
            .as_deref()
            .unwrap_or_default()
            .contains(word)
}
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn report_for<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> &'a DeadSymbolReport {
    reports
        .iter()
        .find(|r| r.symbol_name == symbol)
        .unwrap_or_else(|| panic!("no dead-symbol row for {symbol:?}; rows: {reports:#?}"))
}

#[test]
fn autograd_function_contract_holds_when_the_class_is_named_function() {
    // `class Function(torch.autograd.Function)`: the declarer's name equals
    // the member, which the extractor's self-reference skip drops.
    let (extractions, resolution) = resolve(&[(
        "fn.py",
        "import torch\n\nclass Function(torch.autograd.Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n    @staticmethod\n    def backward(ctx, g):\n        return g\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);
    for method in ["Function.forward", "Function.backward"] {
        let report = report_for(&reports, method);
        assert!(names_contract(report, "autograd"), "{method}: {report:?}");
    }
}

#[test]
fn nn_module_contract_holds_when_the_class_is_named_module() {
    let (extractions, resolution) = resolve(&[(
        "net.py",
        "import torch.nn as nn\n\nclass Module(nn.Module):\n    def forward(self, x):\n        return x\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);
    let report = report_for(&reports, "Module.forward");
    assert!(names_contract(report, "torch"), "{report:?}");
}

#[test]
fn autograd_function_contract_holds_through_an_import_alias() {
    let (extractions, resolution) = resolve(&[(
        "fn.py",
        "from torch import autograd as ag\n\nclass F(ag.Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);
    let report = report_for(&reports, "F.forward");
    assert!(names_contract(report, "autograd"), "{report:?}");
}

#[test]
fn autograd_function_contract_holds_for_parenthesized_and_subscripted_base() {
    for base in ["(torch.autograd.Function)", "torch.autograd.Function[int]"] {
        let src = format!(
            "import torch\n\nclass F({base}):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n"
        );
        let (extractions, resolution) = resolve(&[("fn.py", src.as_str())]);
        let reports = analyze_liveness(&extractions, &resolution);
        let report = report_for(&reports, "F.forward");
        assert!(names_contract(report, "autograd"), "{base}: {report:?}");
    }
}

#[test]
fn a_local_dotted_base_with_a_framework_shaped_tail_inherits_nothing() {
    let (extractions, resolution) = resolve(&[
        ("local/__init__.py", ""),
        ("local/autograd.py", "class Function:\n    pass\n"),
        (
            "t.py",
            "from local import autograd\n\nclass T(autograd.Function):\n    def forward(self, x):\n        return x\n",
        ),
    ]);
    let reports = analyze_liveness(&extractions, &resolution);
    let reason = report_for(&reports, "T.forward")
        .exemption_reason
        .clone()
        .unwrap_or_default();
    assert!(!reason.contains("torch"), "{reason:?}");
}

#[test]
fn a_class_whose_only_base_is_unknown_is_not_confidently_dead() {
    // `make_base()` / `*mixins` / `namedtuple(...)` name a base this corpus
    // cannot see. An uncalled method of a class with a base the corpus does
    // not declare is published at the external-supertype tier, not as safe to
    // delete; the opaque base is exactly such a base.
    for head in [
        "class C(make_base()):",
        "class C(*mixins):",
        "class C(namedtuple('C', 'a b')):",
    ] {
        let src = format!(
            "from collections import namedtuple\nmixins = ()\ndef make_base():\n    return object\n\n{head}\n    def do_GET(self):\n        return 1\n"
        );
        let (extractions, resolution) = resolve(&[("c.py", src.as_str())]);
        let reports = analyze_liveness(&extractions, &resolution);
        let report = report_for(&reports, "C.do_GET");
        assert!(
            !earned_confident_tier(report),
            "{head}: method of an unknown-base class is act-on dead: {report:?}"
        );
    }
}

#[test]
fn the_named_external_base_tier_is_unchanged_by_an_opaque_sibling_base() {
    // A named external base beside an opaque one keeps its tier.
    let (extractions, resolution) = resolve(&[(
        "c.py",
        "import somelib\nmixins = ()\n\nclass C(somelib.Base, *mixins):\n    def hook(self):\n        return 1\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);
    let report = report_for(&reports, "C.hook");
    assert!(!earned_confident_tier(report), "{report:?}");
}

#[test]
fn the_opaque_marker_never_leaks_into_reports_or_resolution() {
    let src = "mixins = ()\ndef make_base():\n    return object\nclass Base:\n    def m(self):\n        return 1\nclass C(*mixins, Base):\n    def m(self):\n        return super().m()\nclass D(make_base()):\n    def unused(self):\n        return 2\n";
    let (extractions, resolution) = resolve(&[("c.py", src)]);
    let reports = analyze_liveness(&extractions, &resolution);
    let shown = format!("{reports:?}");
    assert!(!shown.contains(OPAQUE_BASE), "report leak: {shown}");
    let shown = format!("{resolution:?}");
    assert!(!shown.contains(OPAQUE_BASE), "resolution leak: {shown}");
}

#[test]
fn a_class_literally_named_opaque_in_another_language_is_not_swallowed() {
    // TS/Ruby/Kotlin cannot name `<opaque>`, but a base the other languages
    // record verbatim must never equal the marker; a TS template or JSX
    // heritage text with angle brackets must not turn into one either.
    let (extractions, _) = resolve(&[(
        "w.ts",
        "class A extends Base<opaque> {}\nclass B extends (mixin(Base)) {}\n",
    )]);
    for ext in &extractions {
        for r in &ext.references {
            assert_ne!(r.name, OPAQUE_BASE);
        }
    }
}
