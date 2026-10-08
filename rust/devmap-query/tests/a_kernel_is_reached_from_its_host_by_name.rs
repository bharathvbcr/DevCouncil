//! A Metal kernel's blast radius reaches the Rust that dispatches it by name.
//!
//! Measured 2026-10-06 on tessl (generation 1129): `devmap_search
//! encoder_attn_rows_h256_r16_g32` returned nothing, because the kernel is
//! stamped by `ENC_ROWS_KERNEL(…)`; and `devmap_impact` of the plain kernel
//! `mlp_silu` returned nothing, because the only thing that names it is the
//! string in `rt.pipeline("mlp_silu")`. Kernel blast radius in tessl — and in
//! ojas and qd-metal, which dispatch tessl's kernels the same way — was being
//! answered by `rg` over `pipeline("…")` strings.
//!
//! The corpus below holds both kernel shapes and three host shapes: the
//! literal at the runtime call, the literal handed to a wrapper, and the
//! literal in a dispatch-table row that a variable later carries to the call.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const KERNELS: &str = include_str!("../../devmap-extract/tests/data/stamped_kernels.metal");

/// A second library that also declares `plain_scale` — a tuning copy beside
/// the real kernels. Which of the two a string loads is not in the source.
const TUNE: &str = "kernel void plain_scale(device float *out [[buffer(0)]],
                       uint gid [[thread_position_in_grid]])
{
    out[gid] = 0.0f;
}

kernel void tune_only(device float *out [[buffer(0)]],
                      uint gid [[thread_position_in_grid]])
{
    out[gid] = 1.0f;
}

kernel void const_private(device float *out [[buffer(0)]]) { out[0] = 2.0f; }
kernel void const_public(device float *out [[buffer(0)]]) { out[0] = 3.0f; }
kernel void assoc_kernel(device float *out [[buffer(0)]]) { out[0] = 4.0f; }
";

const HOST: &str = r#"
pub fn scale(rt: &Runtime) {
    let p = rt.pipeline("rows_h256_r16");
}

pub fn gate(rt: &Runtime) {
    let p = pipeline(rt, "gate_f32", "gate");
}

fn rows_for(dim: usize) -> Option<&'static str> {
    match dim {
        512 => Some("rows_h512_r32"),
        _ => None,
    }
}

pub fn rows(rt: &Runtime, dim: usize) {
    let entry = rows_for(dim).unwrap();
    let p = rt.pipeline(entry);
}

pub fn tuned(rt: &Runtime) {
    let p = rt.pipeline("plain_scale");
    let q = rt.pipeline("tune_only");
}

pub fn label() -> &'static str {
    "twice"
}

const PRIVATE_KERNEL: &str = "const_private";
pub const PUBLIC_KERNEL: &str = "const_public";

pub fn via_private(rt: &Runtime) {
    let p = rt.pipeline(PRIVATE_KERNEL);
}

pub fn via_public(rt: &Runtime) {
    let p = rt.pipeline(PUBLIC_KERNEL);
}

impl Runtime {
    const ASSOC: &'static str = "assoc_kernel";

    pub fn via_assoc(&self) {
        let p = self.pipeline(Self::ASSOC);
    }
}
"#;

fn extractions() -> Vec<Extraction> {
    vec![
        extract_file("kernels/stamped_kernels.metal", KERNELS),
        extract_file("kernels/tune/tune.metal", TUNE),
        extract_file("src/host.rs", HOST),
    ]
}

fn store() -> Store {
    let extractions = extractions();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn reached(store: &Store, target: &str, depth: usize) -> Vec<String> {
    let answer = StoreQueryEngine::new(store)
        .impact(Request {
            query: target.to_string(),
            token_budget: 16_000,
            min_confidence: 0.0,
            max_depth: depth,
        })
        .unwrap();
    let mut sources: Vec<String> = answer
        .items
        .iter()
        .map(|edge| edge.source_symbol.clone())
        .collect();
    sources.sort();
    sources.dedup();
    sources
}

#[test]
fn a_stamped_kernel_is_reached_from_the_string_that_dispatches_it() {
    let store = store();
    assert_eq!(
        reached(&store, "kernels/stamped_kernels.metal::rows_h256_r16", 1),
        vec!["src/host.rs::scale".to_string()]
    );
    assert_eq!(
        reached(&store, "kernels/stamped_kernels.metal::gate_f32", 1),
        vec!["src/host.rs::gate".to_string()],
        "a literal handed to a wrapper names the kernel as surely as one at the runtime call"
    );
}

#[test]
fn a_kernel_named_in_a_dispatch_table_reaches_the_dispatching_function() {
    let store = store();
    assert_eq!(
        reached(&store, "kernels/stamped_kernels.metal::rows_h512_r32", 2),
        vec!["src/host.rs::rows".to_string(), "src/host.rs::rows_for".to_string()],
        "the table row names the kernel, and the function that reads the table calls it"
    );
}

/// A name held in a constant reaches the function that dispatches the
/// constant: directly for a private const (not a symbol of its own), through
/// the const for a public one, and through `Self::` for an associated const.
#[test]
fn a_kernel_named_by_a_constant_reaches_the_function_that_reads_it() {
    let store = store();
    assert_eq!(
        reached(&store, "kernels/tune/tune.metal::const_private", 1),
        vec!["src/host.rs::via_private".to_string()]
    );
    assert_eq!(
        reached(&store, "kernels/tune/tune.metal::const_public", 1),
        vec!["src/host.rs::PUBLIC_KERNEL".to_string()]
    );
    assert!(
        reached(&store, "kernels/tune/tune.metal::const_public", 2)
            .contains(&"src/host.rs::via_public".to_string()),
        "the public const's readers are one hop further"
    );
    assert_eq!(
        reached(&store, "kernels/tune/tune.metal::assoc_kernel", 1),
        vec!["src/host.rs::Runtime.via_assoc".to_string()]
    );
}

/// A change to the stamping macro reaches every kernel it stamps and, through
/// each, the Rust that dispatches it.
#[test]
fn a_macro_edit_reaches_the_rust_hosts_of_every_kernel_it_stamps() {
    let store = store();
    let reached = reached(&store, "kernels/stamped_kernels.metal::ROWS_KERNEL", 3);
    for expected in [
        "kernels/stamped_kernels.metal::rows_h256_r16",
        "kernels/stamped_kernels.metal::rows_h512_r32",
        "src/host.rs::scale",
        "src/host.rs::rows_for",
    ] {
        assert!(
            reached.contains(&expected.to_string()),
            "{expected}: {reached:?}"
        );
    }
}

/// The controls. A name two libraries declare is ambiguous and links to
/// neither; a name only one declares links even beside it; and a string naming
/// a shader's private helper links to nothing, because the host cannot look a
/// helper up.
#[test]
fn only_a_unique_entry_point_is_a_target() {
    let store = store();
    assert_eq!(
        reached(&store, "kernels/stamped_kernels.metal::plain_scale", 1),
        Vec::<String>::new()
    );
    assert_eq!(
        reached(&store, "kernels/tune/tune.metal::plain_scale", 1),
        Vec::<String>::new()
    );
    assert_eq!(
        reached(&store, "kernels/tune/tune.metal::tune_only", 1),
        vec!["src/host.rs::tuned".to_string()]
    );
    assert!(
        !reached(&store, "kernels/stamped_kernels.metal::twice", 1)
            .contains(&"src/host.rs::label".to_string()),
        "a helper is not an entry point"
    );
}

/// A literal that names no kernel is not filed as a failed attribution.
#[test]
fn an_unmatched_name_string_is_not_an_unresolved_reference() {
    let extractions = extractions();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let names: Vec<&str> = resolution
        .unresolved
        .iter()
        .map(|row| row.callee_name.as_str())
        .collect();
    for literal in ["twice", "gate", "plain_scale"] {
        assert!(!names.contains(&literal), "{literal}: {names:?}");
    }
}

/// Metal is reported as Metal, not folded into C++.
#[test]
fn resolution_rate_reports_metal_under_its_own_name() {
    let extractions = extractions();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let rate = devmap_analyze::resolution_rate(&extractions, &resolution);
    let metal = rate
        .by_language
        .get("metal")
        .unwrap_or_else(|| panic!("no metal row: {:?}", rate.by_language.keys()));
    assert!(metal.extracts_calls, "{metal:?}");
    assert!(metal.resolved_sites > 0, "{metal:?}");
    assert!(
        !rate.by_language.contains_key("cpp"),
        "no C++ file is in this corpus: {:?}",
        rate.by_language.keys()
    );
}
