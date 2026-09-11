//! X42 — a call on an implicit receiver dispatches on the *enclosing type*.
//!
//! `self.m()`, `cls.m()`, `this.m()` and a Go method's declared receiver all
//! name a member of the type the call is written inside. The resolver had no
//! rung that knew that. What it had instead:
//!
//! * rung 2c, which matches the **bare name** against every symbol the file
//!   declares and requires the count to be exactly one — so two classes in one
//!   file each declaring `run` made `self.run()` unresolvable, even though the
//!   enclosing class answers it outright; and
//! * rung 2a, which runs *first* and consults `import_bindings` **without
//!   looking at the receiver at all** — so `self.run()` in a file carrying
//!   `from helpers import run` bound to `helpers.run`, at DETERMINISTIC. That
//!   is a confidently wrong edge, and it also hands the real method one fewer
//!   caller than it has.
//!
//! The evidence for the new rung is the receiver's type, which is why it is the
//! `ReceiverType` rung and not a new one: `self` inside `class C` *is* a `C`,
//! and the extractor already records which type declares each method
//! (`parent_symbol`) and which types each type extends (`ReferenceKind::
//! Heritage`). Where the enclosing type does not declare the method, its
//! supertypes are walked — that is still the receiver's type, one link out.
//!
//! Ambiguity abstains, as everywhere else here: a diamond that reaches two
//! declarations of one name resolves to neither.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

/// Every `Calls` edge out of `caller`, as `(target file, target symbol, rung)`.
fn calls_from(result: &ResolutionResult, caller: &str) -> Vec<(String, String, String)> {
    let mut rows: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == caller)
        .map(|edge| {
            (
                edge.target_file.clone(),
                edge.target_symbol.clone(),
                format!("{:?}", edge.evidence.expect("resolver evidence").kind),
            )
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

/// Two classes in one file, each declaring `run`. The bare-name count is two,
/// so the same-file rung abstains — but `self` inside `Second` is a `Second`.
#[test]
fn two_classes_in_one_file_do_not_make_an_implicit_receiver_ambiguous() {
    let (_, result) = resolve(&[(
        "svc.py",
        "class First:\n    def run(self):\n        return 1\n\n\n\
         class Second:\n    def run(self):\n        return 2\n\n    \
         def go(self):\n        return self.run()\n",
    )]);

    assert_eq!(
        calls_from(&result, "svc.py::Second.go"),
        vec![(
            "svc.py".to_string(),
            "svc.py::Second.run".to_string(),
            "ReceiverType".to_string()
        )],
        "`self` inside `Second` is a `Second`; `First.run` is a different method \
         with the same name"
    );
}

/// The confidently-wrong shape. An import binding of the same bare name is not
/// what `self.run()` refers to, in any language in this family.
#[test]
fn an_import_of_the_same_name_does_not_capture_an_implicit_receiver_call() {
    let (_, result) = resolve(&[
        ("helpers.py", "def run():\n    return 0\n"),
        (
            "svc.py",
            "from helpers import run\n\n\nclass Service:\n    def run(self):\n        \
             return 1\n\n    def go(self):\n        return self.run()\n",
        ),
    ]);

    assert_eq!(
        calls_from(&result, "svc.py::Service.go"),
        vec![(
            "svc.py".to_string(),
            "svc.py::Service.run".to_string(),
            "ReceiverType".to_string()
        )],
        "`self.run()` is `Service.run`; binding it to the imported `helpers.run` \
         is a confident edge to a function the code demonstrably does not call"
    );
}

/// One link out along the heritage chain is still the receiver's type, and the
/// base class is in another file.
#[test]
fn an_inherited_method_resolves_through_the_declared_base_class() {
    let (_, result) = resolve(&[
        (
            "inherit/base.py",
            "class Base:\n    def helper(self):\n        return 1\n",
        ),
        (
            "inherit/child.py",
            "from inherit.base import Base\n\n\nclass Child(Base):\n    def run(self):\n        \
             return self.helper()\n",
        ),
    ]);

    assert_eq!(
        calls_from(&result, "inherit/child.py::Child.run"),
        vec![(
            "inherit/base.py".to_string(),
            "inherit/base.py::Base.helper".to_string(),
            "ReceiverType".to_string()
        )],
        "`Child` extends `Base`, and `Base` declares `helper` — the receiver's \
         type names the target outright"
    );
}

/// The abstention. Two bases each declaring the name is an ambiguity the
/// receiver's type cannot resolve, and picking one would be a guess.
#[test]
fn a_diamond_that_reaches_two_declarations_resolves_to_neither() {
    let (_, result) = resolve(&[(
        "mix.py",
        "class Left:\n    def shared(self):\n        return 1\n\n\n\
         class Right:\n    def shared(self):\n        return 2\n\n\n\
         class Both(Left, Right):\n    def go(self):\n        return self.shared()\n",
    )]);

    let rows = calls_from(&result, "mix.py::Both.go");
    assert!(
        !rows
            .iter()
            .any(|(_, _, rung)| rung == "ReceiverType" || rung == "SameFile"),
        "two bases declare `shared` and nothing chooses between them; a \
         deterministic edge here would be a guess. got {rows:?}"
    );
}

/// `super().m()` names the base, not the enclosing type — even where both
/// declare the name, which is the whole reason the call is written.
#[test]
fn a_super_call_names_the_base_and_not_the_overriding_method() {
    let (_, result) = resolve(&[(
        "over.py",
        "class Base:\n    def setup(self):\n        return 1\n\n\n\
         class Child(Base):\n    def setup(self):\n        return super().setup()\n",
    )]);

    let rows = calls_from(&result, "over.py::Child.setup");
    assert!(
        !rows
            .iter()
            .any(|(_, symbol, _)| symbol == "over.py::Child.setup"),
        "`super().setup()` is the one call that certainly does *not* mean the \
         overriding method it is written inside; a self-edge here fabricates a \
         caller and shields `Child.setup` from the dead-code pass. got {rows:?}"
    );
}

/// A Go method's receiver is declared in its signature, so `s.M()` inside
/// `func (s *Server) N()` is as determined as `self.m()` is.
#[test]
fn a_go_receiver_dispatches_on_the_type_its_signature_declares() {
    let (_, result) = resolve(&[(
        "svc/server.go",
        "package svc\n\ntype Server struct{}\n\n\
         func (s *Server) start() int { return 1 }\n\n\
         func (s *Server) Run() int { return s.start() }\n",
    )]);

    assert_eq!(
        calls_from(&result, "svc/server.go::Server.Run"),
        vec![(
            "svc/server.go".to_string(),
            "svc/server.go::Server.start".to_string(),
            "ReceiverType".to_string()
        )],
        "`s` is declared `*Server` in the signature above the call"
    );
}

/// Rust's `self` inside an `impl` block, with a second `impl` in the same file
/// declaring the same method name — the shape rung 2c's bare-name count
/// abstains on.
#[test]
fn rust_self_dispatches_on_the_impl_block_it_is_written_in() {
    let (_, result) = resolve(&[(
        "lib.rs",
        "pub struct First;\npub struct Second;\n\n\
         impl First {\n    fn step(&self) -> u32 {\n        1\n    }\n}\n\n\
         impl Second {\n    fn step(&self) -> u32 {\n        2\n    }\n\n    \
         pub fn go(&self) -> u32 {\n        self.step()\n    }\n}\n",
    )]);

    assert_eq!(
        calls_from(&result, "lib.rs::Second.go"),
        vec![(
            "lib.rs".to_string(),
            "lib.rs::Second.step".to_string(),
            "ReceiverType".to_string()
        )],
        "`self` inside `impl Second` is a `Second`"
    );
}

/// A scope that rebinds `self`. The enclosing-type rung is a *default*, and it
/// must not override evidence the scope wrote down: `self = Other()` says what
/// `self` is here, as plainly as a constructor assignment says it for any other
/// name. The rung is ordered after the receiver-binding rung for exactly this,
/// and the property that matters is that the call still resolves to **one**
/// method rather than fanning out to both candidates.
#[test]
fn an_explicit_rebinding_of_self_outranks_the_enclosing_type() {
    let (_, result) = resolve(&[(
        "odd.py",
        "class Other:\n    def run(self):\n        return 1\n\n\n\
         class Service:\n    def run(self):\n        return 2\n\n    \
         def go(self):\n        self = Other()\n        return self.run()\n",
    )]);

    let methods: Vec<_> = calls_from(&result, "odd.py::Service.go")
        .into_iter()
        .filter(|(_, symbol, _)| symbol.ends_with(".run"))
        .collect();
    assert_eq!(
        methods,
        vec![(
            "odd.py".to_string(),
            "odd.py::Other.run".to_string(),
            "ReceiverType".to_string()
        )],
        "the scope constructed an `Other` and bound `self` to it; that is \
         written evidence about this call, and exactly one method answers it"
    );
}

/// A class that declares the same method twice — the last one wins in Python,
/// and the resolver has no business preferring either. The edge must still name
/// the enclosing class rather than escaping to another file.
#[test]
fn a_class_that_declares_a_method_twice_stays_inside_its_own_type() {
    let (_, result) = resolve(&[
        ("other.py", "def run():\n    return 0\n"),
        (
            "dup.py",
            "class Service:\n    def run(self):\n        return 1\n\n    \
             def run(self):\n        return 2\n\n    \
             def go(self):\n        return self.run()\n",
        ),
    ]);

    let rows = calls_from(&result, "dup.py::Service.go");
    assert!(
        rows.iter().all(|(file, _, _)| file == "dup.py"),
        "a duplicate declaration is an ambiguity inside one class, not a reason \
         to bind the call to an unrelated function in another file. got {rows:?}"
    );
}
