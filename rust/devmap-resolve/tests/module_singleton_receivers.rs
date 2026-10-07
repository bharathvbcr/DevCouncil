//! A module-level instance dispatches its methods wherever it is imported.
//!
//! `export const draftQuestionsService = new DraftQuestionsServiceClass()` is
//! the most common service shape in a TypeScript frontend, and Python's
//! `service = Service()` is the same fact in another spelling: one value, typed
//! by its initializer, declared once at module scope and called from everywhere
//! else. The receiver's type is recorded in the declaring file; a caller in
//! another file reaches it only through an import binding. Before this, every
//! such call landed in the ledger as `uninferred_receiver` and the method had
//! no callers at all — on scholarlm, `draftQuestionsService.updateAnswer` had
//! eight call sites and an empty caller list.
//!
//! The value/module split this needed found a second defect, pinned in the
//! "not a module handle" cases below: the module-member rung read every named
//! import as a module, so `svc.helper()` bound to a free `function helper`
//! sharing the module.

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

const TS_SERVICE: &str = "\
export class SvcClass {
  zzupdate(field: string) {}
  zzclear() {}
}
export class Other {
  zzupdate(field: string) {}
}
export const svc = new SvcClass();
";

#[test]
fn a_ts_singleton_imported_by_name_dispatches_to_its_class() {
    let modal = "\
import { svc } from './service';
export function Modal() {
  svc.zzupdate('a');
}
";
    let targets = calls_from(
        &[("src/service.ts", TS_SERVICE), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzupdate"),
        "an imported singleton is typed by its initializer, got {targets:?}"
    );
    assert!(
        !targets.iter().any(|t| t.contains("Other.zzupdate")),
        "the namesake on an unrelated class must not be reached, got {targets:?}"
    );
}

#[test]
fn a_ts_singleton_used_in_its_own_file_dispatches_to_its_class() {
    let source = "\
export class SvcClass {
  zzupdate() {}
}
export const svc = new SvcClass();
export function reset() {
  svc.zzupdate();
}
";
    let targets = calls_from(&[("src/service.ts", source)], "src/service.ts::reset");
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzupdate"),
        "got {targets:?}"
    );
}

#[test]
fn an_aliased_import_keeps_the_singletons_type() {
    let modal = "\
import { svc as questions } from './service';
export function Modal() {
  questions.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.ts", TS_SERVICE), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_singleton_reexported_through_a_barrel_keeps_its_type() {
    let barrel = "export { svc } from './service';\n";
    let modal = "\
import { svc } from './index';
export function Modal() {
  svc.zzclear();
}
";
    let targets = calls_from(
        &[
            ("src/service.ts", TS_SERVICE),
            ("src/index.ts", barrel),
            ("src/Modal.tsx", modal),
        ],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_js_singleton_imported_by_name_dispatches_to_its_class() {
    let service = "\
export class SvcClass {
  zzclear() {}
}
export const svc = new SvcClass();
";
    let modal = "\
import { svc } from './service.js';
export function Modal() {
  svc.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.js", service), ("src/Modal.jsx", modal)],
        "src/Modal.jsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.js::SvcClass.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_python_module_instance_imported_by_name_dispatches_to_its_class() {
    let service = "\
class Service:
    def zzrun(self):
        pass

service = Service()
";
    let caller = "\
from pkg.service import service

def handle():
    service.zzrun()
";
    let targets = calls_from(
        &[("pkg/service.py", service), ("pkg/caller.py", caller)],
        "pkg/caller.py::handle",
    );
    assert!(
        targets.iter().any(|t| t == "pkg/service.py::Service.zzrun"),
        "got {targets:?}"
    );
}

#[test]
fn a_singleton_built_from_injected_dependencies_is_typed_by_its_own_class() {
    // Every argument carries the declared name as `assigned_to`; the outermost
    // expression is the constructor, and it alone types the value.
    let service = "\
export class Dep {
  zzupdate() {}
}
export class SvcClass {
  zzupdate() {}
}
export const svc = new SvcClass(new Dep(), makeOther());
";
    let modal = "\
import { svc } from './service';
export function Modal() {
  svc.zzupdate();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzupdate"),
        "got {targets:?}"
    );
    assert!(
        !targets.iter().any(|t| t.ends_with("Dep.zzupdate")),
        "the argument is not the value, got {targets:?}"
    );
}

#[test]
fn a_singleton_renamed_by_a_barrel_keeps_its_type() {
    let barrel = "export { svc as questions } from './service';\n";
    let modal = "\
import { questions } from './index';
export function Modal() {
  questions.zzclear();
}
";
    let targets = calls_from(
        &[
            ("src/service.ts", TS_SERVICE),
            ("src/index.ts", barrel),
            ("src/Modal.tsx", modal),
        ],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_singleton_reached_through_a_namespace_import_dispatches_to_its_class() {
    let modal = "\
import * as service from './service';
export function Modal() {
  service.svc.zzclear();
}
";
    let targets = calls_from(
        &[("src/service.ts", TS_SERVICE), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzclear"),
        "got {targets:?}"
    );
}

#[test]
fn a_python_singleton_reached_through_its_module_dispatches_to_its_class() {
    let service = "\
class Service:
    def zzrun(self):
        pass

service = Service()
";
    let caller = "\
from pkg import service_mod

def handle():
    service_mod.service.zzrun()
";
    let targets = calls_from(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/service_mod.py", service),
            ("app/caller.py", caller),
        ],
        "app/caller.py::handle",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "pkg/service_mod.py::Service.zzrun"),
        "got {targets:?}"
    );
}

#[test]
fn a_singleton_is_also_a_member_reference_target() {
    // A method passed by value — `onClick={svc.zzclear}` — is a use, and it
    // resolves through the same typed receiver.
    let modal = "\
import { svc } from './service';
export function Modal() {
  register(svc.zzclear);
}
";
    let extractions: Vec<Extraction> = [("src/service.ts", TS_SERVICE), ("src/Modal.tsx", modal)]
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let edges = resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges;
    assert!(
        edges
            .iter()
            .any(|edge| edge.source_symbol == "src/Modal.tsx::Modal"
                && edge.target_symbol == "src/service.ts::SvcClass.zzclear"),
        "got {:?}",
        edges
            .iter()
            .filter(|e| e.source_symbol == "src/Modal.tsx::Modal")
            .map(|e| (&e.target_symbol, format!("{:?}", e.edge_kind)))
            .collect::<Vec<_>>()
    );
}

// ---- the binding must not leak ----

#[test]
fn a_local_that_shadows_the_import_is_not_the_singleton() {
    let modal = "\
import { svc } from './service';
export function Modal() {
  const svc = makeOther();
  svc.zzupdate('a');
}
";
    let targets = calls_from(
        &[("src/service.ts", TS_SERVICE), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        !targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzupdate"),
        "a local `svc` shadows the import and its type is unknown, got {targets:?}"
    );
}

#[test]
fn a_parameter_that_shadows_the_import_is_not_the_singleton() {
    let modal = "\
import { svc } from './service';
export function Modal(svc) {
  svc.zzupdate('a');
}
";
    let targets = calls_from(
        &[("src/service.ts", TS_SERVICE), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        !targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzupdate"),
        "a parameter `svc` shadows the import, got {targets:?}"
    );
}

#[test]
fn a_binding_reassigned_to_another_class_abstains() {
    let service = "\
export class SvcClass {
  zzupdate() {}
}
export class Other {
  zzupdate() {}
}
export let svc = new SvcClass();
svc = new Other();
";
    let modal = "\
import { svc } from './service';
export function Modal() {
  svc.zzupdate();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        !targets
            .iter()
            .any(|t| t.ends_with("SvcClass.zzupdate") || t.ends_with("Other.zzupdate")),
        "two classes claim the binding; the answer is unknown, got {targets:?}"
    );
}

#[test]
fn a_function_scope_reassignment_of_the_singleton_abstains() {
    let service = "\
export class SvcClass {
  zzupdate() {}
}
export class Other {
  zzupdate() {}
}
export let svc = new SvcClass();
export function swap() {
  svc = new Other();
}
";
    let modal = "\
import { svc } from './service';
export function Modal() {
  svc.zzupdate();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        !targets
            .iter()
            .any(|t| t.ends_with("SvcClass.zzupdate") || t.ends_with("Other.zzupdate")),
        "`swap` reassigns the module binding, so its type is not one class, got {targets:?}"
    );
}

#[test]
fn a_function_local_of_the_same_name_does_not_type_the_export() {
    // `svc` at module scope comes from a factory whose type is unknown; a
    // function-local `svc` in the same file is a different binding and must not
    // lend the export its type.
    let service = "\
export class SvcClass {
  zzupdate() {}
}
export const svc = makeSvc();
export function build() {
  const svc = new SvcClass();
  return svc;
}
";
    let modal = "\
import { svc } from './service';
export function Modal() {
  svc.zzupdate();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        !targets.iter().any(|t| t.ends_with("SvcClass.zzupdate")),
        "a function local is not the module binding, got {targets:?}"
    );
}

#[test]
fn a_named_value_import_is_not_a_module_handle() {
    // `svc` is a value. `svc.zzhelper()` is a method on whatever `svc` is — it
    // is never the free function `zzhelper` that happens to share its module.
    let service = "\
export const svc = makeSvc();
export function zzhelper() {}
";
    let modal = "\
import { svc } from './service';
export function Modal() {
  svc.zzhelper();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        !targets.iter().any(|t| t == "src/service.ts::zzhelper"),
        "a value import is not its module, got {targets:?}"
    );
}

#[test]
fn a_python_named_value_import_is_not_a_module_handle() {
    let init = "\
obj = make()

def zzhelper():
    pass
";
    let caller = "\
from pkg import obj

def handle():
    obj.zzhelper()
";
    let targets = calls_from(
        &[("pkg/__init__.py", init), ("app/caller.py", caller)],
        "app/caller.py::handle",
    );
    assert!(
        !targets.iter().any(|t| t == "pkg/__init__.py::zzhelper"),
        "a value import is not its module, got {targets:?}"
    );
}

#[test]
fn a_python_submodule_import_is_still_a_module_handle() {
    // The other half: `from pkg import cmd` where `cmd` is a submodule file
    // keeps resolving `cmd.zzrun()` into that file.
    let cmd = "def zzrun():\n    pass\n";
    let caller = "\
from pkg import cmd

def handle():
    cmd.zzrun()
";
    let targets = calls_from(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/cmd.py", cmd),
            ("app/caller.py", caller),
        ],
        "app/caller.py::handle",
    );
    assert!(
        targets.iter().any(|t| t == "pkg/cmd.py::zzrun"),
        "a submodule import is a module handle, got {targets:?}"
    );
}

#[test]
fn a_namespace_import_is_still_a_module_handle() {
    let service = "export function zzhelper() {}\n";
    let modal = "\
import * as service from './service';
export function Modal() {
  service.zzhelper();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets.iter().any(|t| t == "src/service.ts::zzhelper"),
        "got {targets:?}"
    );
}

// ---- object-literal namespaces keep their edges ----
//
// Found by diffing scholarlm's graph before and after the value/module split:
// `export const traceService = { appendTraceEvent, … }` and
// `export const debugLogger = { info() {…} }` are namespaces built from the
// module's own declarations, and `traceService.appendTraceEvent()` really is a
// call to that function.

#[test]
fn a_shorthand_object_namespace_reaches_the_function_it_names() {
    let service = "\
export function zzappend() {}
export function zzclear() {}
export const traceService = {
  zzappend,
  zzclear,
};
";
    let caller = "\
import { traceService } from './trace';
export function Panel() {
  traceService.zzappend();
}
";
    let targets = calls_from(
        &[("src/trace.ts", service), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        targets.iter().any(|t| t == "src/trace.ts::zzappend"),
        "got {targets:?}"
    );
}

#[test]
fn an_object_literal_method_is_reached_through_the_object() {
    let core = "\
export const logger = {
  zzinfo(message: string) {
    record(message);
  },
};
";
    let caller = "\
import { logger } from './core';
export function Panel() {
  logger.zzinfo('x');
}
";
    let targets = calls_from(
        &[("src/core.ts", core), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        targets.iter().any(|t| t == "src/core.ts::zzinfo"),
        "got {targets:?}"
    );
}

#[test]
fn an_object_namespace_does_not_admit_a_name_it_does_not_spell() {
    let service = "\
export function zzappend() {}
export function zzother() {}
export const traceService = { zzappend };
";
    let caller = "\
import { traceService } from './trace';
export function Panel() {
  traceService.zzother();
}
";
    let targets = calls_from(
        &[("src/trace.ts", service), ("src/Panel.tsx", caller)],
        "src/Panel.tsx::Panel",
    );
    assert!(
        !targets.iter().any(|t| t == "src/trace.ts::zzother"),
        "the literal never names zzother, got {targets:?}"
    );
}

#[test]
fn a_factory_returning_an_object_literal_keeps_its_method_edge() {
    // Found on scholarlm: `registryAdapter(run).tagsForDigest()` where the
    // imported function returns a literal declaring `tagsForDigest`.
    let adapter = "\
export function registryAdapter(run) {
  return {
    zztags(ref) {
      return run(ref);
    },
  };
}
";
    let test = "\
import { registryAdapter } from './adapter.mjs';
export function check() {
  return registryAdapter(() => 'x').zztags(1);
}
";
    let targets = calls_from(
        &[("ops/adapter.mjs", adapter), ("ops/check.js", test)],
        "ops/check.js::check",
    );
    assert!(
        targets
            .iter()
            .any(|t| t.ends_with("registryAdapter.zztags")),
        "got {targets:?}"
    );
}

#[test]
fn a_rust_module_path_is_not_read_through_a_same_named_function_import() {
    // Found on DevCouncil's own dc-regress: `pub use blast::{blast, Report}`
    // binds the function `blast`, while `blast::Report` walks the module.
    let blast = "pub fn blast() {}\npub struct ZzReport;\n";
    let lib = "\
mod blast;
pub use blast::{blast, ZzReport};
pub fn affected() -> Vec<blast::ZzReport> {
    Vec::new()
}
";
    let extractions: Vec<Extraction> = [("crate/src/blast.rs", blast), ("crate/src/lib.rs", lib)]
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let edges = resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges;
    assert!(
        edges
            .iter()
            .any(|edge| edge.target_symbol == "crate/src/blast.rs::ZzReport"
                && edge.source_file == "crate/src/lib.rs"),
        "got {:?}",
        edges
            .iter()
            .filter(|edge| edge.source_file == "crate/src/lib.rs")
            .map(|edge| (&edge.source_symbol, &edge.target_symbol))
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_imported_enum_member_is_still_referenced() {
    let types = "\
export enum Page {
  ZzReview = 'review',
}
";
    let caller = "\
import { Page } from './types';
export function nav() {
  return go(Page.ZzReview);
}
";
    let extractions: Vec<Extraction> = [("src/types.ts", types), ("src/nav.ts", caller)]
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let edges = resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges;
    let from_nav: Vec<_> = edges
        .iter()
        .filter(|edge| edge.source_symbol == "src/nav.ts::nav")
        .map(|edge| edge.target_symbol.clone())
        .collect();
    // An imported type is the static shape, not a value: its member keeps the
    // edge it had before value imports were told apart from modules.
    assert!(
        from_nav.iter().any(|t| t.starts_with("src/types.ts::")),
        "got {from_nav:?}"
    );
}

#[test]
fn an_import_of_a_class_is_not_an_instance_of_it() {
    // `import { SvcClass }` names the class itself; `SvcClass.zzupdate()` is a
    // static call shape and must not be widened into "an instance of SvcClass".
    let service = "\
export class SvcClass {
  static zzbuild() {}
}
";
    let modal = "\
import { SvcClass } from './service';
export function Modal() {
  SvcClass.zzbuild();
}
";
    let targets = calls_from(
        &[("src/service.ts", service), ("src/Modal.tsx", modal)],
        "src/Modal.tsx::Modal",
    );
    assert!(
        targets
            .iter()
            .any(|t| t == "src/service.ts::SvcClass.zzbuild"),
        "static call on an imported class, got {targets:?}"
    );
}
