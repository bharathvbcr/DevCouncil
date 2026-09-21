//! A receiver held through a smart pointer still calls the inner type's method.
//!
//! Measured on `qwen-decision` before this test existed: `Service.wait_tick`
//! was published by `devmap_dead_symbols` at **confidence 0.9 with no
//! exemption** — the tier whose contract is "safe to act on" — while being
//! called 22 lines below its own definition, in the same file, inside a live
//! reaper loop:
//!
//! ```ignore
//! pub fn run_idle_reaper(service: Arc<Service>, stop: Arc<AtomicBool>) {
//!     loop {
//!         service.wait_tick(tick);   // <- this call produced no edge
//! ```
//!
//! The type argument was dropped at *extraction*: `Arc<Service>` was recorded
//! as the declared type `"Arc"`. That is a plain identifier, so it passed every
//! admissibility check on the way down and the resolver simply looked for a
//! type named `Arc` and found none. Nothing was degraded, nothing was
//! truncated, and no coverage gap fired — the graph asserted a live loop's wait
//! was dead.
//!
//! The rule is `Deref`, not "unwrap generics": `Arc<T>`, `Rc<T>` and `Box<T>`
//! are `Deref<Target = T>`, so the method really is the inner type's. The
//! negative half of this file is the more important half — `Vec<T>`,
//! `Option<T>`, `Mutex<T>` and `RwLock<T>` own their own methods, and a rule
//! that unwrapped those would fabricate edges instead of finding them.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
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

fn calls_wait_tick(result: &ResolutionResult, from: &str) -> bool {
    result.edges.iter().any(|edge| {
        edge.source_symbol == from
            && edge
                .target_symbol
                .rsplit(['.', ':'])
                .next()
                .is_some_and(|tail| tail == "wait_tick")
    })
}

/// One file, so every edge asserted here is a *same-file* resolution and no
/// import or cross-file rung can be what makes it pass.
fn service_source(receiver_decl: &str, body: &str) -> String {
    format!(
        "use std::sync::Arc;\n\
         use std::rc::Rc;\n\
         use std::sync::Mutex;\n\
         use std::sync::RwLock;\n\
         \n\
         pub struct Service {{ n: u32 }}\n\
         \n\
         impl Service {{\n\
         \x20   fn wait_tick(&self, tick: u32) -> u32 {{ tick + self.n }}\n\
         \x20   pub fn probe({receiver_decl}) -> u32 {{ {body} }}\n\
         }}\n"
    )
}

#[test]
fn a_method_called_through_arc_rc_or_box_resolves_to_the_inner_type() {
    for decl in [
        "service: Arc<Service>, tick: u32",
        "service: Rc<Service>, tick: u32",
        "service: Box<Service>, tick: u32",
        "service: &Arc<Service>, tick: u32",
        "service: std::sync::Arc<Service>, tick: u32",
    ] {
        let source = service_source(decl, "service.wait_tick(tick)");
        let result = resolve(&[("svc.rs", &source)]);
        assert!(
            calls_wait_tick(&result, "svc.rs::Service.probe"),
            "`{decl}` is Deref<Target = Service>, so `service.wait_tick(tick)` \
             calls `Service::wait_tick`. Without this edge the method is \
             published as dead at 0.9 with no exemption. Edges: {:?}",
            result.edges
        );
    }
}

/// The half that keeps the fix from becoming a fabricator.
///
/// `Vec<Service>` has no `wait_tick`; asserting one would be a graph that
/// invents edges rather than one that misses them, which is strictly worse —
/// a missing edge makes something look dead, an invented edge makes something
/// dead look live.
#[test]
fn a_wrapper_that_owns_its_own_methods_is_not_unwrapped() {
    for decl in [
        "service: Vec<Service>, tick: u32",
        "service: Option<Service>, tick: u32",
        "service: Mutex<Service>, tick: u32",
        "service: RwLock<Service>, tick: u32",
        // `Arc<Mutex<Service>>` derefs to `Mutex<Service>`, and a caller must
        // `.lock()` before any `Service` method is reachable. Stopping at
        // `Mutex` is the correct answer, not a partial one.
        "service: Arc<Mutex<Service>>, tick: u32",
    ] {
        let source = service_source(decl, "service.wait_tick(tick)");
        let result = resolve(&[("svc.rs", &source)]);
        assert!(
            !calls_wait_tick(&result, "svc.rs::Service.probe"),
            "`{decl}` does not deref to `Service`, so no edge to \
             `Service::wait_tick` may be asserted. Edges: {:?}",
            result.edges
        );
    }
}

/// `let service = Service { n: 0 };` — the shape Rust had no arm for at all.
///
/// `struct_expression` appeared nowhere in the extractor, so the resolver's own
/// `T{..}` initializer branch was unreachable for Rust: it was written for this
/// shape and never fed by it. Go's `composite_literal` has been recorded since
/// SC17, which is what makes this a gap in one grammar rather than a policy.
#[test]
fn a_binding_initialised_by_a_struct_literal_is_typed_by_it() {
    let source = "pub struct Service { n: u32 }\n\
                  impl Service {\n\
                  \x20   fn wait_tick(&self, tick: u32) -> u32 { tick + self.n }\n\
                  \x20   pub fn probe(tick: u32) -> u32 {\n\
                  \x20       let service = Service { n: 0 };\n\
                  \x20       service.wait_tick(tick)\n\
                  \x20   }\n\
                  }\n";
    let result = resolve(&[("svc.rs", source)]);
    assert!(
        calls_wait_tick(&result, "svc.rs::Service.probe"),
        "a struct literal types the binding it initialises, exactly as \
         `Service::new()` already did. Edges: {:?}",
        result.edges
    );
}

/// Nesting a caller controls must not become runtime a caller controls.
///
/// `rust_type_name` recurses per wrapper level, and this rule adds a second
/// reason to descend. The first draft decided whether to follow a wrapper by
/// *trial-calling* `rust_type_name` on the argument and then letting the caller
/// recurse into the same node — two visits per level, so `Arc<Arc<…>>` cost
/// 2^depth node visits. Bounded by the depth cap at 2^16, which is bounded and
/// still absurd. The decision is a constant-time look at the node's kind now,
/// and this is the test that says so in wall-clock rather than in prose.
///
/// The 10,000-level case is not hypothetical: the comment on `go_type_name`
/// records a measured stack overflow that aborted `devmap build` with exit 134
/// on a ~10 KB file, four orders of magnitude under `MAX_SOURCE_BYTES`.
#[test]
fn pathological_wrapper_nesting_is_bounded() {
    for depth in [8usize, 17, 64, 10_000] {
        let source = format!(
            "pub struct Service {{ n: u32 }}\n\
             impl Service {{\n\
             \x20   fn wait_tick(&self, tick: u32) -> u32 {{ tick + self.n }}\n\
             \x20   pub fn probe(service: {}Service{}, tick: u32) -> u32 {{ service.wait_tick(tick) }}\n\
             }}\n",
            "Arc<".repeat(depth),
            ">".repeat(depth),
        );
        let started = std::time::Instant::now();
        let result = resolve(&[("svc.rs", &source)]);
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "{depth} levels of `Arc<…>` took {elapsed:?}"
        );
        // Past the depth bound the type is simply not recovered. That is the
        // safe direction and the one the existing bound already chose: a lost
        // qualification, never a guessed one. The only thing asserted here is
        // that it terminates without overflowing.
        drop(result);
    }
}

/// `Arc<Arc<Service>>` really is a `Service` — two derefs, both transparent.
#[test]
fn stacked_transparent_wrappers_unwrap_all_the_way() {
    let source = service_source(
        "service: Arc<Box<Service>>, tick: u32",
        "service.wait_tick(tick)",
    );
    let result = resolve(&[("svc.rs", &source)]);
    assert!(
        calls_wait_tick(&result, "svc.rs::Service.probe"),
        "`Arc<Box<Service>>` derefs to `Box<Service>` and then to `Service`. \
         Edges: {:?}",
        result.edges
    );
}

/// The C++ half of the same rule, through `operator->`.
///
/// Added with the Rust half rather than after it: `c_type_name`'s fallback arm
/// reached the template's `name` field first and answered `shared_ptr`, which
/// is the identical defect in a different grammar. A fix applied to one
/// language and not the other is how the two come to disagree about what a
/// receiver is.
#[test]
fn a_cpp_smart_pointer_receiver_resolves_to_the_pointee() {
    let source = "struct Service { int wait_tick(int tick) { return tick; } };\n\
                  int probe(std::shared_ptr<Service> service, int tick) {\n\
                  \x20   return service->wait_tick(tick);\n\
                  }\n";
    let result = resolve(&[("svc.cpp", source)]);
    assert!(
        calls_wait_tick(&result, "svc.cpp::probe"),
        "`std::shared_ptr<Service>` forwards member access to `Service`, so \
         the out-of-line member is reachable and not dead. Edges: {:?}",
        result.edges
    );
}

/// And the C++ negative control: `weak_ptr` has no `operator->`.
///
/// Uses the *same* corpus shape as the positive case above, with an inline
/// method definition, on purpose. The first draft of both declared
/// `int wait_tick(int tick);` with no body, so no method symbol existed at all
/// and this control passed without exercising anything — it would have gone on
/// passing if `weak_ptr` had been in the allowlist.
#[test]
fn a_cpp_weak_pointer_is_not_unwrapped() {
    let source = "struct Service { int wait_tick(int tick) { return tick; } };\n\
                  int probe(std::weak_ptr<Service> service, int tick) {\n\
                  \x20   return service->wait_tick(tick);\n\
                  }\n";
    let result = resolve(&[("svc.cpp", source)]);
    assert!(
        !calls_wait_tick(&result, "svc.cpp::probe"),
        "`weak_ptr` must be `.lock()`ed into a `shared_ptr` first, so its \
         members are its own. Edges: {:?}",
        result.edges
    );
}
