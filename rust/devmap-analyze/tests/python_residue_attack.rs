//! Adversarial probes of liveness for `lane/py-residue` @ 9e8f2a26: an opaque
//! base counted as an external supertype, and the autograd / nn.Module
//! contract re-derived from heritage segments. A failing test is a BREAK; a
//! test named `finding_` states a documented trade-off.

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

fn rows(files: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let (extractions, resolution) = resolve(files);
    analyze_liveness(&extractions, &resolution)
}

fn find<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|r| r.symbol_name == symbol)
}

fn contract(reports: &[DeadSymbolReport], symbol: &str, word: &str) -> bool {
    find(reports, symbol).is_some_and(|r| {
        r.is_exempt && r.exemption_reason.as_deref().unwrap_or_default().contains(word)
    })
}

fn not_exempt(reports: &[DeadSymbolReport], symbol: &str) -> bool {
    find(reports, symbol).is_some_and(|r| !r.is_exempt)
}

// ------------------------------------------------- opaque base, dead methods

#[test]
fn local_make_basedead_method_private_method_is_still_reported() {
    let r = rows(&[(
        "c.py",
        "def make_base():\n    return object\n\nclass D(make_base()):\n    def dead_method(self):\n        return 1\n",
    )]);
    assert!(not_exempt(&r, "D.dead_method"), "{:?}", find(&r, "D.dead_method"));
}

#[test]
fn finding_local_make_basedead_method_private_method_keeps_the_confident_tier() {
    // A class with a local, fully known base reports its dead private method
    // at the confident tier; an opaque base lowers it. Report the delta.
    let known = rows(&[(
        "c.py",
        "class B:\n    pass\n\nclass D(B):\n    def dead_method(self):\n        return 1\n",
    )]);
    let opaque = rows(&[(
        "c.py",
        "def make_base():\n    return object\n\nclass D(make_base()):\n    def dead_method(self):\n        return 1\n",
    )]);
    let k = find(&known, "D.dead_method").expect("known-base row").confidence;
    let o = find(&opaque, "D.dead_method").expect("opaque-base row").confidence;
    assert!(o >= k, "opaque base lowered a dead private method from {k} to {o}");
}

#[test]
fn star_args_basedead_method_private_method_is_still_reported() {
    let r = rows(&[(
        "c.py",
        "class D(*mixins):\n    def dead_method(self):\n        return 1\n",
    )]);
    assert!(not_exempt(&r, "D.dead_method"), "{:?}", find(&r, "D.dead_method"));
}

#[test]
fn opaque_base_on_a_nested_class_does_not_exempt_the_outer_classs_methods() {
    let r = rows(&[(
        "c.py",
        "def make_base():\n    return object\n\nclass Outer:\n    class Inner(make_base()):\n        pass\n\n    def dead_method(self):\n        return 1\n",
    )]);
    assert!(not_exempt(&r, "Outer.dead_method"), "{:?}", find(&r, "Outer.dead_method"));
}

#[test]
fn opaque_base_on_a_subclass_does_not_exempt_the_unrelated_sibling() {
    let r = rows(&[(
        "c.py",
        "def make_base():\n    return object\n\nclass A(make_base()):\n    pass\n\nclass B:\n    def dead_method(self):\n        return 1\n",
    )]);
    assert!(not_exempt(&r, "B.dead_method"), "{:?}", find(&r, "B.dead_method"));
}

#[test]
fn opaque_base_two_levels_up_confines_exemption_to_descendants() {
    // `Mid(make_base())`, `Leaf(Mid)`: Leaf's methods may be called by the
    // opaque base's code; a class that merely sits beside them may not.
    let r = rows(&[(
        "c.py",
        "def make_base():\n    return object\n\nclass Mid(make_base()):\n    pass\n\nclass Leaf(Mid):\n    def dead_method(self):\n        return 1\n\nclass Alone:\n    def dead_method(self):\n        return 1\n",
    )]);
    assert!(not_exempt(&r, "Alone.dead_method"), "{:?}", find(&r, "Alone.dead_method"));
}

#[test]
fn a_module_level_function_beside_an_opaque_base_class_is_stilldead_method() {
    let r = rows(&[(
        "c.py",
        "def make_base():\n    return object\n\nclass D(make_base()):\n    pass\n\ndef dead_fn():\n    return 1\n",
    )]);
    assert!(
        find(&r, "dead_fn").is_some_and(|row| !row.is_exempt),
        "{:?}",
        find(&r, "dead_fn")
    );
}

// ------------------------------------------------------ framework contracts

#[test]
fn autograd_function_reached_through_a_local_reexport_module() {
    let r = rows(&[
        ("compat.py", "from torch.autograd import Function\n"),
        (
            "fn.py",
            "from compat import Function\n\nclass F(Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n    @staticmethod\n    def backward(ctx, g):\n        return g\n",
        ),
    ]);
    for method in ["F.forward", "F.backward"] {
        assert!(
            find(&r, method).is_some_and(|row| row.is_exempt || row.confidence < 0.9),
            "{method} is reported at the confident tier with no exemption: {:?}",
            find(&r, method)
        );
    }
}

#[test]
fn autograd_function_through_a_dotted_alias_reexport() {
    let r = rows(&[
        ("compat.py", "import torch\nautograd = torch.autograd\n"),
        (
            "fn.py",
            "import compat\n\nclass F(compat.autograd.Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n",
        ),
    ]);
    assert!(
        find(&r, "F.forward").is_some_and(|row| row.is_exempt || row.confidence < 0.9),
        "{:?}",
        find(&r, "F.forward")
    );
}

#[test]
fn a_local_class_named_function_is_not_the_autograd_contract() {
    let r = rows(&[(
        "fn.py",
        "import torch\n\nclass Function:\n    pass\n\nclass F(Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n",
    )]);
    assert!(
        !contract(&r, "F.forward", "autograd"),
        "{:?}",
        find(&r, "F.forward")
    );
}

#[test]
fn a_module_that_rebinds_torch_is_not_the_contract() {
    let r = rows(&[(
        "fn.py",
        "import torch\n\nclass Fake:\n    pass\ntorch = Fake()\n\nclass F(torch.autograd.Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n",
    )]);
    assert!(
        !contract(&r, "F.forward", "autograd"),
        "{:?}",
        find(&r, "F.forward")
    );
}

#[test]
fn a_corpus_module_named_torch_with_an_autograd_function_is_not_the_contract() {
    let r = rows(&[
        ("torch/__init__.py", ""),
        ("torch/autograd.py", "class Function:\n    pass\n"),
        (
            "fn.py",
            "from torch import autograd\n\nclass F(autograd.Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n",
        ),
    ]);
    assert!(
        !contract(&r, "F.forward", "autograd"),
        "{:?}",
        find(&r, "F.forward")
    );
}

#[test]
fn an_autograd_function_only_exempts_its_dispatched_methods() {
    let r = rows(&[(
        "fn.py",
        "import torch\n\nclass F(torch.autograd.Function):\n    @staticmethod\n    def forward(ctx, x):\n        return x\n    @staticmethod\n    def helper_never_called(x):\n        return x\n",
    )]);
    assert!(
        !contract(&r, "F.helper_never_called", "autograd"),
        "{:?}",
        find(&r, "F.helper_never_called")
    );
}

#[test]
fn a_function_scoped_import_of_torch_reaches_the_contract_or_stays_unexempt_never_wrong() {
    let r = rows(&[(
        "fn.py",
        "def make():\n    import torch\n    class F(torch.autograd.Function):\n        @staticmethod\n        def forward(ctx, x):\n            return x\n    return F\n",
    )]);
    // Recall check: the base is torch's whatever scope imports it.
    assert!(
        find(&r, "F.forward").is_some_and(|row| row.is_exempt || row.confidence < 0.9),
        "{:?}",
        find(&r, "F.forward")
    );
}

#[test]
fn nn_module_through_a_subscripted_and_parenthesised_base() {
    let r = rows(&[(
        "net.py",
        "import torch.nn as nn\n\nclass A((nn.Module)):\n    def forward(self, x):\n        return x\n\nclass B(nn.Module, metaclass=type):\n    def forward(self, x):\n        return x\n",
    )]);
    assert!(contract(&r, "A.forward", "torch"), "{:?}", find(&r, "A.forward"));
    assert!(contract(&r, "B.forward", "torch"), "{:?}", find(&r, "B.forward"));
}

#[test]
fn nn_module_exempts_forward_but_not_a_privatedead_method_method() {
    let r = rows(&[(
        "net.py",
        "import torch.nn as nn\n\nclass A(nn.Module):\n    def forward(self, x):\n        return x\n    def dead_method(self):\n        return 1\n",
    )]);
    assert!(
        !contract(&r, "A.dead_method", "torch"),
        "{:?}",
        find(&r, "A.dead_method")
    );
}

#[test]
fn nn_module_in_a_class_that_a_second_local_class_of_the_same_name_shadows() {
    // `class M(nn.Module)` then `class M:` plain: forward on the second is not
    // a torch method.
    let r = rows(&[(
        "net.py",
        "import torch.nn as nn\n\nclass M(nn.Module):\n    def forward(self, x):\n        return x\n\nclass M:\n    def forward(self, x):\n        return x\n",
    )]);
    let plain: Vec<_> = r.iter().filter(|row| row.symbol_name == "M.forward").collect();
    assert!(
        plain.iter().any(|row| !row.is_exempt) || plain.len() <= 1,
        "{plain:?}"
    );
}
