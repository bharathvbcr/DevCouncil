//! A framework dispatches some methods of a subclass. Which ones is knowable
//! only from the base, and only when the base is one we actually know.
//!
//! Measured on `qwen-decision`: `_FusedLinearCE.forward` and
//! `_FusedLinearCE.backward` were published by `devmap_dead_symbols` at
//! **confidence 0.9 with no exemption reason** — the tier whose contract is
//! "safe to act on". They are `torch.autograd.Function` methods, and torch
//! calls them through `.apply()` from inside the framework, so no static
//! analyser sees a caller. Acting on the report deletes an autograd backward.
//!
//! The existing heritage rule (W1.2) cannot cover this, for a precise reason:
//! `supertypes_by_type` is built from resolved `Extends`/`Implements` **edges**,
//! and an unresolvable supertype produces no edge at all — measured,
//! `class _FusedLinearCE(torch.autograd.Function)` yields three heritage
//! *references* and **zero** heritage edges. To every edge-based index the
//! class has no supertypes whatever, and its methods look ordinary.
//!
//! # The rule this file does *not* implement, and why
//!
//! The first attempt was structural and framework-agnostic: exempt every method
//! of a class all of whose supertypes are absent from the corpus, on the theory
//! that an unread base means an unread dispatcher. It is a tempting rule
//! because it needs no allowlist, and it is wrong.
//! `test_runtime_entry_points_are_exempt_without_exempting_their_file` in
//! `devmap-cli` caught it: `class Widget extends HTMLElement` made
//! `Widget.unusedMethod` exempt, and that method is genuinely dead.
//!
//! The two cases are structurally identical — an uncalled method on a class
//! with an out-of-corpus base — so no signal available here separates them.
//! What separates them is knowledge of the *contract*: `HTMLElement` dispatches
//! `connectedCallback`, not `unusedMethod`, and `wiring.rs` has encoded that
//! precise fact for years. So this joins that registry instead of standing a
//! second, guessier one beside it. The cost is honest and stated: a framework
//! the registry does not know gets no exemption, and its overrides stay
//! candidates.
//!
//! # What shipped for the unknown bases (2026-10-10)
//!
//! What was rejected is the *exemption*. For a base the registry does not know
//! (`known_dispatch_contract` in `wiring.rs` says which it does), an uncalled
//! method is published at 0.4 `external_supertype`, naming the base: visible,
//! never exempt, out of the act-on tier. The corpus record measured it — 23
//! false and 2 unsure confident rows left the act-on tier (HTMLParser handlers,
//! watchdog and `sys.meta_path` hooks, CodeMirror widget methods), no true one.
//! A registry-known base keeps the precision this file is about: `autograd`'s
//! `helper_nobody_calls` and `HTMLElement`'s `unusedMethod` stay confident.
//!
//! "Known" is decided on the base as the file's imports name it
//! (`torch.nn.Module`, `react.Component`), never on its spelling: a bare
//! `Component` from another library inherited React's contract until a review
//! caught it, and so did every hook the lists left out (`formResetCallback`,
//! React's `UNSAFE_` names). A known base keeps its *other* methods in the act-on
//! tier, so its hook list has to be the standard's whole list.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
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

/// Strict on purpose.
///
/// An earlier draft looked up bare method names (`forward`) while the reports
/// are keyed by the qualified identity (`_FusedLinearCE.forward`), and skipped
/// the assertion when the lookup missed. Three of four tests passed green
/// having asserted nothing at all. A lookup that cannot find its subject is a
/// broken test, not a satisfied one.
fn report_for<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> &'a DeadSymbolReport {
    reports
        .iter()
        .find(|r| r.symbol_name == symbol)
        .unwrap_or_else(|| panic!("no dead-symbol row for {symbol:?}; rows present: {reports:#?}"))
}

/// `confidently dead`: the evidence reached the tier whose contract is "safe to
/// act on" — [`earned_confident_tier`], so a language that has not earned the
/// tier yet (Python, since the 2026-10-10 corpus record) still answers for its
/// evidence rather than for the table.
fn confidently_dead(report: &DeadSymbolReport) -> bool {
    earned_confident_tier(report)
}

const AUTOGRAD: &str = "import torch\n\
                        \n\
                        \n\
                        class _FusedLinearCE(torch.autograd.Function):\n\
                        \x20   @staticmethod\n\
                        \x20   def forward(ctx, x, w):\n\
                        \x20       return x @ w\n\
                        \x20   @staticmethod\n\
                        \x20   def backward(ctx, grad):\n\
                        \x20       return grad, grad\n\
                        \x20   def helper_nobody_calls(self):\n\
                        \x20       return 1\n";

/// The exact shape from the audit.
#[test]
fn a_torch_autograd_method_is_not_reported_actionably_dead() {
    let (extractions, resolution) = resolve(&[("fused_ce.py", AUTOGRAD)]);
    let reports = analyze_liveness(&extractions, &resolution);

    for method in ["_FusedLinearCE.forward", "_FusedLinearCE.backward"] {
        let report = report_for(&reports, method);
        assert!(
            !confidently_dead(report),
            "`{method}` is dispatched by autograd through `.apply()`, from code \
             that is not in this corpus. Reporting it at {:.2} with reason {:?} \
             is what would have deleted an autograd backward.",
            report.confidence,
            report.exemption_reason
        );
        let reason = report.exemption_reason.as_deref().unwrap_or_default();
        assert!(
            reason.contains("autograd"),
            "the reason must name the contract, so a human can judge it rather \
             than take it on trust. Got {reason:?}"
        );
    }
}

/// The half that keeps the exemption from swallowing the report — and the
/// case the structural rule got wrong.
///
/// `helper_nobody_calls` sits on the *same class*, inside the same framework
/// subclass, and is genuinely dead. A rule keyed on the class rather than on
/// the method would exempt it, which is precisely how `dead_symbols` becomes a
/// list that never says anything — so it must stay reported, never exempt.
///
/// Since 2026-10-10 an uncalled method of a class whose base the corpus does
/// not declare is published at 0.4 `external_supertype` — unless the base's
/// contract is known, as autograd's is. So this stays *confident*: knowing the
/// contract is exactly what lets the other methods be judged.
#[test]
fn an_ordinary_method_on_the_same_framework_class_stays_actionable() {
    let (extractions, resolution) = resolve(&[("fused_ce.py", AUTOGRAD)]);
    let reports = analyze_liveness(&extractions, &resolution);

    let report = report_for(&reports, "_FusedLinearCE.helper_nobody_calls");
    assert!(
        confidently_dead(report),
        "nothing calls `helper_nobody_calls`, autograd does not dispatch it, \
         and it must stay actionable. Exempting it is the over-exemption that \
         `test_runtime_entry_points_are_exempt_without_exempting_their_file` \
         exists to catch. Got exempt={} confidence={:.2} reason={:?}",
        report.is_exempt,
        report.confidence,
        report.exemption_reason
    );
}

/// `nn.Module.forward` is the same contract through a different entry point.
#[test]
fn a_torch_module_forward_is_dispatched_by_call() {
    let (extractions, resolution) = resolve(&[(
        "net.py",
        "import torch.nn as nn\n\
         \n\
         \n\
         class Net(nn.Module):\n\
         \x20   def forward(self, x):\n\
         \x20       return x\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);
    let report = report_for(&reports, "Net.forward");
    assert!(
        !confidently_dead(report),
        "`Module.__call__` invokes `forward`; nothing in the repository does. \
         Got {report:?}"
    );
}

/// The qualifier is load-bearing.
///
/// A local class named `Module` or `Function` must not inherit torch's dispatch
/// contract. This is why the registry matches `nn.Module` and
/// `autograd.Function` rather than the bare tail — and it is the difference
/// between naming a known contract and guessing from a common word.
#[test]
fn a_local_class_with_a_framework_shaped_name_inherits_nothing() {
    let (extractions, resolution) = resolve(&[(
        "local.py",
        "class Module:\n\
         \x20   pass\n\
         \n\
         \n\
         class Thing(Module):\n\
         \x20   def forward(self, x):\n\
         \x20       return x\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);

    let report = report_for(&reports, "Thing.forward");
    let reason = report.exemption_reason.as_deref().unwrap_or_default();
    assert!(
        !reason.contains("torch"),
        "`Module` here is declared two lines up, in this file. Inheriting \
         torch's contract from the spelling of a name is the guess this design \
         refuses to make. Got {reason:?}"
    );
}

/// A plain class with no base is untouched by any of this.
#[test]
fn a_class_with_no_base_is_still_an_ordinary_candidate() {
    let (extractions, resolution) = resolve(&[(
        "plain.py",
        "class Plain:\n\
         \x20   def orphan(self):\n\
         \x20       return 1\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);

    let report = report_for(&reports, "Plain.orphan");
    assert!(
        confidently_dead(report),
        "`Plain` has no base at all. Reason: {:?}",
        report.exemption_reason
    );
}

/// Whether `symbol` is spared: exempt, or never a candidate at all.
fn spared(reports: &[DeadSymbolReport], symbol: &str) -> bool {
    reports
        .iter()
        .find(|r| r.symbol_name == symbol)
        .is_none_or(|r| r.is_exempt)
}

/// The registry matches the library, not a spelling.
///
/// `known_dispatch_contract` matched a bare `Component`, so a class extending a
/// `Component` from any library inherited React's contract, and its uncalled
/// methods stayed in the act-on tier — the unqualified-name guess
/// `a_local_class_with_a_framework_shaped_name_inherits_nothing` refuses.
#[test]
fn a_component_imported_from_another_library_is_not_reacts() {
    let (extractions, resolution) = resolve(&[(
        "widget.ts",
        "import { Component } from 'some-lib';\n\
         \n\
         class Widget extends Component {\n\
         \x20 onHostEvent(): void {}\n\
         }\n\
         \n\
         new Widget();\n",
    )]);
    let reports = analyze_liveness(&extractions, &resolution);
    let report = report_for(&reports, "Widget.onHostEvent");
    assert!(
        !confidently_dead(report),
        "`some-lib`'s Component may call `onHostEvent`; nothing here says it \
         does not. Got {report:?}"
    );
    assert!(
        report
            .exemption_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with(EXTERNAL_SUPERTYPE)),
        "it is unconfirmed because of its base, and says so: {report:?}"
    );
}

/// React's contract is still known, however the file names it.
#[test]
fn a_react_component_keeps_its_contract_by_name_or_namespace() {
    for (path, source) in [
        (
            "named.tsx",
            "import { Component } from 'react';\n\
             \n\
             class Panel extends Component {\n\
             \x20 render() { return null; }\n\
             \x20 helperNobodyCalls(): number { return 1; }\n\
             }\n\
             \n\
             new Panel();\n",
        ),
        (
            "namespace.tsx",
            "import React from 'react';\n\
             \n\
             class Panel extends React.Component {\n\
             \x20 render() { return null; }\n\
             \x20 helperNobodyCalls(): number { return 1; }\n\
             }\n\
             \n\
             new Panel();\n",
        ),
    ] {
        let (extractions, resolution) = resolve(&[(path, source)]);
        let reports = analyze_liveness(&extractions, &resolution);
        assert!(spared(&reports, "Panel.render"), "{path}: {reports:#?}");
        let report = report_for(&reports, "Panel.helperNobodyCalls");
        assert!(
            confidently_dead(report),
            "{path}: React's contract is known, so a method it does not \
             dispatch stays actionable. Got {report:?}"
        );
    }
}

/// The hook lists the carve-out rests on are the standards' whole lists.
///
/// A known base keeps its other methods confident, so a hook missing from the
/// list is a live method published as safe to delete. `formResetCallback` and
/// the other form-associated reactions (HTML standard, custom elements), and
/// React's `UNSAFE_` lifecycle names, were missing.
#[test]
fn every_hook_a_known_base_dispatches_is_spared() {
    let (extractions, resolution) = resolve(&[
        (
            "field.ts",
            "class Field extends HTMLElement {\n\
             \x20 formAssociatedCallback(): void {}\n\
             \x20 formDisabledCallback(): void {}\n\
             \x20 formResetCallback(): void {}\n\
             \x20 formStateRestoreCallback(): void {}\n\
             \x20 connectedMoveCallback(): void {}\n\
             }\n\
             customElements.define('x-field', Field);\n",
        ),
        (
            "legacy.tsx",
            "import { Component } from 'react';\n\
             \n\
             class Legacy extends Component {\n\
             \x20 UNSAFE_componentWillMount(): void {}\n\
             \x20 UNSAFE_componentWillReceiveProps(): void {}\n\
             \x20 UNSAFE_componentWillUpdate(): void {}\n\
             \x20 render() { return null; }\n\
             }\n\
             \n\
             new Legacy();\n",
        ),
        (
            "net.py",
            "import torch.nn as nn\n\
             \n\
             \n\
             class Net(nn.Module):\n\
             \x20   def forward(self, x):\n\
             \x20       return x\n\
             \x20   def extra_repr(self):\n\
             \x20       return 'net'\n\
             \x20   def get_extra_state(self):\n\
             \x20       return {}\n\
             \x20   def set_extra_state(self, state):\n\
             \x20       pass\n",
        ),
    ]);
    let reports = analyze_liveness(&extractions, &resolution);
    for hook in [
        "Field.formAssociatedCallback",
        "Field.formDisabledCallback",
        "Field.formResetCallback",
        "Field.formStateRestoreCallback",
        "Field.connectedMoveCallback",
        "Legacy.UNSAFE_componentWillMount",
        "Legacy.UNSAFE_componentWillReceiveProps",
        "Legacy.UNSAFE_componentWillUpdate",
        "Net.extra_repr",
        "Net.get_extra_state",
        "Net.set_extra_state",
    ] {
        let report = reports.iter().find(|r| r.symbol_name == hook);
        assert!(
            report.is_none_or(|r| r.is_exempt),
            "the framework calls `{hook}`: {report:?}"
        );
    }
}
