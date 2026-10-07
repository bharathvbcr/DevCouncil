//! A value built by a factory is typed by what the factory says it returns.
//!
//! `w := NewWorker()` in Go, `const svc = createService()` in a TypeScript
//! service module and `w = make_worker()` with `-> Worker` in Python are each
//! the idiomatic constructor of their language, and the value they build had
//! no type: the initializer was a call to a *function*, and only a class name
//! typed a receiver. Every method called on such a value had no caller. On
//! scholarlm, 74 Go package vars are built this way.
//!
//! The extractor records the return type the callee's declaration writes
//! (`ExtractedSymbol::return_type`); the resolver finds the callee by the same
//! rungs the call ladder trusts and keeps only a single indexed class.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

fn calls_from(files: &[(&str, &str)], source: &str) -> Vec<String> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges
        .into_iter()
        .filter(|edge| edge.source_symbol == source && format!("{:?}", edge.edge_kind) == "Calls")
        .map(|edge| edge.target_symbol)
        .collect()
}

const GO_REGISTRY: &str = "\
package reg

type Registry struct{}

func NewRegistry() *Registry { return &Registry{} }

func Open() (*Registry, error) { return &Registry{}, nil }

func (r *Registry) Zzadd() {}
";

#[test]
fn a_go_local_built_by_a_package_factory_dispatches_to_its_type() {
    let caller = "\
package reg

func Register() {
	w := NewRegistry()
	w.Zzadd()
}
";
    let targets = calls_from(
        &[("reg/registry.go", GO_REGISTRY), ("reg/register.go", caller)],
        "reg/register.go::Register",
    );
    assert!(
        targets.iter().any(|t| t == "reg/registry.go::Registry.Zzadd"),
        "got {targets:?}"
    );
}

#[test]
fn a_go_package_var_built_by_a_factory_types_every_file_of_its_package() {
    let decl = "\
package reg

var defaultRegistry = NewRegistry()
";
    let caller = "\
package reg

func Register() {
	defaultRegistry.Zzadd()
}
";
    let targets = calls_from(
        &[
            ("reg/registry.go", GO_REGISTRY),
            ("reg/default.go", decl),
            ("reg/register.go", caller),
        ],
        "reg/register.go::Register",
    );
    assert!(
        targets.iter().any(|t| t == "reg/registry.go::Registry.Zzadd"),
        "got {targets:?}"
    );
}

#[test]
fn a_go_package_var_built_by_a_composite_literal_types_a_sibling_file() {
    let decl = "\
package reg

var defaultRegistry = &Registry{}
";
    let caller = "\
package reg

func Register() {
	defaultRegistry.Zzadd()
}
";
    let targets = calls_from(
        &[
            ("reg/registry.go", GO_REGISTRY),
            ("reg/default.go", decl),
            ("reg/register.go", caller),
        ],
        "reg/register.go::Register",
    );
    assert!(
        targets.iter().any(|t| t == "reg/registry.go::Registry.Zzadd"),
        "got {targets:?}"
    );
}

#[test]
fn a_go_var_declared_in_a_function_is_typed_by_its_factory() {
    let caller = "\
package reg

func Register() {
	var w = NewRegistry()
	w.Zzadd()
}
";
    let targets = calls_from(
        &[("reg/registry.go", GO_REGISTRY), ("reg/register.go", caller)],
        "reg/register.go::Register",
    );
    assert!(
        targets.iter().any(|t| t == "reg/registry.go::Registry.Zzadd"),
        "got {targets:?}"
    );
}

#[test]
fn a_ts_local_built_by_an_imported_factory_dispatches_to_its_class() {
    let service = "\
export class Svc {
  zzclear() {}
}
export function makeSvc(): Svc {
  return new Svc();
}
";
    let caller = "\
import { makeSvc } from './service';
export function Panel() {
  const s = makeSvc();
  s.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        targets.iter().any(|t| t == "src/service.ts::Svc.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_ts_singleton_built_by_a_factory_types_its_importers() {
    let service = "\
export class Svc {
  zzclear() {}
}
function createService(): Svc {
  return new Svc();
}
export const svc = createService();
";
    let caller = "\
import { svc } from './service';
export function Panel() {
  svc.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        targets.iter().any(|t| t == "src/service.ts::Svc.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_ts_static_factory_types_its_value() {
    let service = "\
export class Svc {
  static create(): Svc {
    return new Svc();
  }
  zzclear() {}
}
";
    let caller = "\
import { Svc } from './service';
export function Panel() {
  const s = Svc.create();
  s.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        targets.iter().any(|t| t == "src/service.ts::Svc.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_python_local_built_by_a_factory_dispatches_to_its_class() {
    let source = "\
class Worker:
    def zzrun(self):
        pass

def make_worker() -> Worker:
    return Worker()

def handle():
    w = make_worker()
    w.zzrun()
";
    let targets = calls_from(&[("pkg/work.py", source)], "pkg/work.py::handle");
    assert!(
        targets.iter().any(|t| t == "pkg/work.py::Worker.zzrun"),
        "got {targets:?}"
    );
}

#[test]
fn a_python_factory_reached_through_its_module_types_the_value() {
    let factories = "\
class Worker:
    def zzrun(self):
        pass

def make() -> 'Worker':
    return Worker()
";
    let caller = "\
from pkg import factories

def handle():
    w = factories.make()
    w.zzrun()
";
    let targets = calls_from(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/factories.py", factories),
            ("app/caller.py", caller),
        ],
        "app/caller.py::handle",
    );
    assert!(
        targets.iter().any(|t| t == "pkg/factories.py::Worker.zzrun"),
        "got {targets:?}"
    );
}

#[test]
fn a_rust_free_factory_and_a_self_returning_associated_fn_type_their_values() {
    let source = "\
pub struct Svc;
impl Svc {
    pub fn open(path: &str) -> Self { Svc }
    pub fn zzrun(&self) {}
}
pub fn make() -> Svc { Svc }
pub fn caller() {
    let a = make();
    a.zzrun();
    let b = Svc::open(\"p\");
    b.zzrun();
}
";
    let extractions = vec![extract_file("src/lib.rs", source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let edges = resolver.resolve_all(&extractions).expect("resolution").edges;
    let hits = edges
        .iter()
        .filter(|edge| {
            edge.source_symbol == "src/lib.rs::caller"
                && edge.target_symbol == "src/lib.rs::Svc.zzrun"
        })
        .count();
    assert!(hits >= 1, "got {:?}", edges);
}

/// Found on scholarlm's `TestExtraProviders`: one test function, one `p` per
/// `t.Run` closure, each built by a different provider factory. The
/// extractor once gave every use of `p` the *first* assignment's initializer,
/// so the IEEE subtest's `p.Search()` was typed as the DBLP provider.
#[test]
fn each_subtest_closure_types_its_own_rebinding_of_a_name() {
    let providers = "\
package search

type DBLP struct{}
type IEEE struct{}

func NewDBLP() *DBLP { return &DBLP{} }
func NewIEEE() *IEEE { return &IEEE{} }

func (d *DBLP) Zzsearch() {}
func (i *IEEE) Zzsearch() {}
";
    let test = "\
package search

import \"testing\"

func TestProviders(t *testing.T) {
	t.Run(\"dblp\", func(t *testing.T) {
		p := NewDBLP()
		p.Zzsearch()
	})
	t.Run(\"ieee\", func(t *testing.T) {
		p := NewIEEE()
		p.Zzsearch()
	})
}
";
    let targets = calls_from(
        &[
            ("search/providers.go", providers),
            ("search/providers_test.go", test),
        ],
        "search/providers_test.go::TestProviders",
    );
    for expected in [
        "search/providers.go::DBLP.Zzsearch",
        "search/providers.go::IEEE.Zzsearch",
    ] {
        assert!(
            targets.iter().any(|t| t == expected),
            "{expected} missing: {targets:?}"
        );
    }
}

/// Found on DevCouncil and GitPulse: `b = b.step()` re-assigns a builder from
/// itself. A position-aware lookup that took the nearest assignment lost the
/// constructor behind it; the step is passed over.
#[test]
fn a_builder_reassigned_from_itself_keeps_its_constructors_type() {
    let source = "\
pub struct Builder;
impl Builder {
    pub fn new() -> Self { Builder }
    pub fn center(self) -> Self { self }
    pub fn zzbuild(&self) {}
}
pub fn make() {
    let mut b = Builder::new();
    b = b.center();
    b.zzbuild();
}
";
    let extractions = vec![extract_file("src/lib.rs", source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let edges = resolver.resolve_all(&extractions).expect("resolution").edges;
    assert!(
        edges.iter().any(|edge| edge.source_symbol == "src/lib.rs::make"
            && edge.target_symbol == "src/lib.rs::Builder.zzbuild"),
        "got {:?}",
        edges
            .iter()
            .filter(|edge| edge.source_symbol == "src/lib.rs::make")
            .map(|edge| &edge.target_symbol)
            .collect::<Vec<_>>()
    );
}

/// Both shapes found on real corpora after the first cut of the rule: a
/// binding named like the first segment of its constructor's path
/// (`let mut lexical = lexical::store::Builder::new()`, DevCouncil's dc-grep)
/// is not a reassignment from itself, and a builder step written across lines
/// (GitPulse's tauri `window = window\n.center()`) is one.
#[test]
fn a_path_sharing_the_bindings_name_and_a_multiline_step_are_read_correctly() {
    let store = "\
pub struct Builder;
impl Builder {
    pub fn new() -> Self { Builder }
    pub fn center(self) -> Self { self }
    pub fn zzadd(&mut self) {}
}
";
    let index = "\
pub fn build() {
    let mut store = store::Builder::new();
    store = store
        .center();
    store.zzadd();
}
";
    let targets = calls_from(
        &[("src/store.rs", store), ("src/index.rs", index), ("src/lib.rs", "mod store;\nmod index;\n")],
        "src/index.rs::build",
    );
    assert!(
        targets.iter().any(|t| t == "src/store.rs::Builder.zzadd"),
        "got {targets:?}"
    );
}

/// Found on GitPulse's vendored muda: `let wrapped = spawn(move || {
/// wrapped.snapshot(); wrapped })` shadows `wrapped`, and the closure moves
/// in the *earlier* one. The right-hand side runs before the new name is bound.
#[test]
fn a_use_inside_a_shadowing_let_reads_the_earlier_binding() {
    let kinds = "\
pub struct Wrapper;
impl Wrapper {
    pub fn new() -> Self { Wrapper }
    pub fn zzsnapshot(&self) {}
}
";
    let test = "\
pub fn project() {
    let wrapped = Wrapper::new();
    let wrapped = std::thread::spawn(move || {
        wrapped.zzsnapshot();
        wrapped
    })
    .join()
    .unwrap();
}
";
    let targets = calls_from(
        &[("src/kinds.rs", kinds), ("src/snapshot.rs", test), ("src/lib.rs", "mod kinds;\nmod snapshot;\n")],
        "src/snapshot.rs::project",
    );
    assert!(
        targets.iter().any(|t| t == "src/kinds.rs::Wrapper.zzsnapshot"),
        "got {targets:?}"
    );
}

/// Go's `:=` is a Rust `let`: the inner `c` is in scope only after its
/// statement, so the closure inside its right-hand side captures the outer `c`.
#[test]
fn a_closure_inside_a_go_short_var_declaration_reads_the_outer_variable() {
    let client = "\
package app

type Client struct{}
type Wrapped struct{}

func NewClient() *Client { return &Client{} }
func Wrap(f func()) *Wrapped { return &Wrapped{} }

func (c *Client) Zzping() {}
func (w *Wrapped) Zzping() {}
";
    let caller = "\
package app

import \"testing\"

func TestPing(t *testing.T) {
	c := NewClient()
	t.Run(\"inner\", func(t *testing.T) {
		c := Wrap(func() { c.Zzping() })
		_ = c
	})
}
";
    let targets = calls_from(
        &[("app/client.go", client), ("app/client_test.go", caller)],
        "app/client_test.go::TestPing",
    );
    assert!(
        targets.iter().any(|t| t == "app/client.go::Client.Zzping"),
        "got {targets:?}"
    );
    assert!(
        !targets.iter().any(|t| t == "app/client.go::Wrapped.Zzping"),
        "the inner `c` is not in scope inside its own initializer: {targets:?}"
    );
}

/// The exception: a JavaScript closure over a *reassigned* variable reads it
/// when it runs, after the assignment, so `s = makeSvc(() => s.zzrun())` calls
/// the new value's method — and a direct `t = wrap(t)` reads the old one.
#[test]
fn a_closure_over_a_reassigned_variable_reads_the_new_value() {
    let svc = "\
export class Svc { zzrun() {} }
export class Old { zzrun() {} zzwrap() {} }
export function makeSvc(cb: () => void): Svc { return new Svc(); }
";
    let caller = "\
import { Svc, Old, makeSvc } from './svc';
export function Panel() {
  let s = new Old();
  s = makeSvc(() => s.zzrun());
  let t = new Old();
  t = makeSvc(t.zzwrap());
}
";
    let targets = calls_from(
        &[("src/svc.ts", svc), ("src/Panel.ts", caller)],
        "src/Panel.ts::Panel",
    );
    assert!(
        targets.iter().any(|t| t == "src/svc.ts::Old.zzwrap"),
        "the direct use reads the old value: {targets:?}"
    );
    assert!(
        !targets.iter().any(|t| t == "src/svc.ts::Old.zzrun"),
        "the closure runs after `s` is reassigned: {targets:?}"
    );
}

/// Found on DevCouncil's dc-evidence and GitPulse's vendored dc-store: a
/// struct literal's field initializer carries the binding's name too, and is
/// nearer the use than the literal's type.
#[test]
fn a_struct_literal_is_typed_by_its_type_not_by_a_fields_initializer() {
    let source = "\
pub struct Scanner<'a> { bytes: &'a [u8], pos: usize }
impl<'a> Scanner<'a> {
    pub fn zzpeek(&self) {}
}
pub fn check(text: &str) {
    let mut scanner = Scanner {
        bytes: text.as_bytes(),
        pos: 0,
    };
    scanner.zzpeek();
}
";
    // A second `Scanner` keeps the index-time class map from answering, so
    // the use is typed by its own binding, as on the corpus that found it.
    let targets = calls_from(
        &[("src/json.rs", source), ("src/other.rs", "pub struct Scanner;\n")],
        "src/json.rs::check",
    );
    assert!(
        targets.iter().any(|t| t == "src/json.rs::Scanner.zzpeek"),
        "got {targets:?}"
    );
}

/// Found on GitPulse's `pipe_drain.rs` and DevCouncil's `protocol.rs`: a
/// chain's outer link (`.expect`, `.with_cancel`) starts where its receiver
/// starts, so it is the *earliest* candidate in its statement. The value is
/// typed by the chain's root, the constructor.
#[test]
fn a_constructor_chain_is_typed_by_its_root() {
    let source = "\
pub struct PipeDrain;
impl PipeDrain {
    pub fn new(cap: usize) -> Result<Self, ()> { Ok(PipeDrain) }
    pub fn zzdrain(&mut self) {}
}
pub fn run() {
    let mut drain = PipeDrain::new(32).expect(\"drain\");
    drain.zzdrain();
}
";
    let targets = calls_from(
        &[("src/pipe.rs", source), ("src/other.rs", "pub struct PipeDrain;\n")],
        "src/pipe.rs::run",
    );
    assert!(
        targets.iter().any(|t| t == "src/pipe.rs::PipeDrain.zzdrain"),
        "got {targets:?}"
    );
}

#[test]
fn a_name_reassigned_on_one_branch_types_nothing() {
    let providers = "\
package search

type DBLP struct{}
type IEEE struct{}

func NewDBLP() *DBLP { return &DBLP{} }
func NewIEEE() *IEEE { return &IEEE{} }

func (d *DBLP) Zzsearch() {}
func (i *IEEE) Zzsearch() {}
";
    let caller = "\
package search

func pick(ieee bool) {
	p := NewDBLP()
	if ieee {
		p = NewIEEE()
	}
	p.Zzsearch()
}
";
    let targets = calls_from(
        &[("search/providers.go", providers), ("search/pick.go", caller)],
        "search/pick.go::pick",
    );
    assert!(
        !targets.iter().any(|t| t.ends_with("Zzsearch")),
        "either branch may have run, got {targets:?}"
    );
}

// ---- refusals ----

#[test]
fn a_go_tuple_result_types_nothing() {
    let caller = "\
package reg

func Register() {
	w, err := Open()
	_ = err
	w.Zzadd()
}
";
    let targets = calls_from(
        &[("reg/registry.go", GO_REGISTRY), ("reg/register.go", caller)],
        "reg/register.go::Register",
    );
    // Whatever else may type `w`, the factory rung must not: `(T, error)`
    // names no single value. The binding is two-target, so nothing does.
    assert!(
        !targets.iter().any(|t| t == "reg/registry.go::Registry.Zzadd"),
        "got {targets:?}"
    );
}

#[test]
fn an_async_factory_is_not_its_awaited_type() {
    let service = "\
export class Svc {
  zzclear() {}
}
export async function loadSvc(): Promise<Svc> {
  return new Svc();
}
";
    let caller = "\
import { loadSvc } from './service';
export function Panel() {
  const s = loadSvc();
  s.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        !targets.iter().any(|t| t == "src/service.ts::Svc.zzclear"),
        "a Promise is not a Svc, got {targets:?}"
    );
}

#[test]
fn a_parameter_named_like_the_factory_is_not_the_factory() {
    let service = "\
export class Svc {
  zzclear() {}
}
export function makeSvc(): Svc {
  return new Svc();
}
";
    let caller = "\
import { makeSvc } from './service';
export function Panel(makeSvc) {
  const s = makeSvc();
  s.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        !targets.iter().any(|t| t == "src/service.ts::Svc.zzclear"),
        "the parameter shadows the import, got {targets:?}"
    );
}

#[test]
fn a_return_type_two_files_declare_types_nothing() {
    let a = "export class Svc { zzclear() {} }\n";
    let b = "export class Svc { zzclear() {} }\n";
    let factory = "\
import { Svc } from './a';
export function makeSvc(): Svc {
  return new Svc();
}
";
    let caller = "\
import { makeSvc } from './factory';
export function Panel() {
  const s = makeSvc();
  s.zzclear();
}
";
    let targets = calls_from(
        &[
            ("src/a.ts", a),
            ("src/b.ts", b),
            ("src/factory.ts", factory),
            ("src/Panel.tsx", caller),
        ],
        "src/Panel.tsx::Panel",
    );
    assert!(
        !targets.iter().any(|t| t.ends_with("Svc.zzclear")),
        "`Svc` names two classes, got {targets:?}"
    );
}

/// Found on scholarlm: one subtest builds `client` from a composite literal, a
/// later one from the package factory, and the corpus declares a `Client` in
/// another package too. The later closure's own `client := NewClient()` is the
/// binding its calls see, and its `Client` is the one its own package declares
/// — the same reading the composite literal beside it gets.
#[test]
fn a_factory_binding_in_a_later_subtest_survives_an_earlier_literal_one() {
    let other = "\
package rpc

type Client struct{}

func (c *Client) Zzdial() {}
";
    let client = "\
package llm

type Client struct{ url string }

func NewClient() *Client { return &Client{} }

func (c *Client) Zzhealth() {}
func (c *Client) Zzembed() {}
";
    let test = "\
package llm

import (
	\"net/http\"
	\"testing\"
)

func TestHelpers(t *testing.T) {
	t.Run(\"literal\", func(t *testing.T) {
		client := &Client{url: \"://bad\"}
		client.Zzhealth()
		client = &Client{url: \"http://x\"}
		client.Zzhealth()
	})
	t.Run(\"factory\", func(t *testing.T) {
		server := serve(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			switch r.URL.Path {
			case \"/a\":
				w.WriteHeader(400)
			default:
				http.NotFound(w, r)
			}
		}))
		defer server.Close()
		client := NewClient()
		client.Zzembed()
	})
}
";
    let targets = calls_from(
        &[
            ("rpc/client.go", other),
            ("llm/client.go", client),
            ("llm/client_test.go", test),
        ],
        "llm/client_test.go::TestHelpers",
    );
    assert!(
        !targets.iter().any(|t| t.starts_with("rpc/")),
        "the other package's `Client` is not this one: {targets:?}"
    );
    for expected in ["llm/client.go::Client.Zzhealth", "llm/client.go::Client.Zzembed"] {
        assert!(
            targets.iter().any(|t| t == expected),
            "{expected} missing: {targets:?}"
        );
    }
}

/// The other half: `llm.NewClient()` returns `llm`'s `Client`, and a caller in
/// a package that declares a `Client` of its own reads that name as its own
/// type. Handing the bare name across the package boundary would dispatch on
/// the wrong one.
#[test]
fn a_factory_type_name_the_callers_package_redeclares_types_nothing() {
    let llm = "\
package llm

type Client struct{}

func NewClient() *Client { return &Client{} }

func (c *Client) Zzembed() {}
";
    let api = "\
package api

import \"example.com/app/llm\"

type Client struct{}

func (c *Client) Zzembed() {}

func Handle() {
	c := llm.NewClient()
	c.Zzembed()
}
";
    let targets = calls_from(
        &[("llm/client.go", llm), ("api/handler.go", api)],
        "api/handler.go::Handle",
    );
    assert!(
        !targets.iter().any(|t| t == "api/handler.go::Client.Zzembed"),
        "`c` is llm's Client, not api's: {targets:?}"
    );
}
