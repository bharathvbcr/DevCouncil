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
kernel void swift_kernel(device float *out [[buffer(0)]]) { out[0] = 5.0f; }
kernel void swift_const(device float *out [[buffer(0)]]) { out[0] = 6.0f; }
kernel void objc_kernel(device float *out [[buffer(0)]]) { out[0] = 7.0f; }
kernel void objc_const(device float *out [[buffer(0)]]) { out[0] = 8.0f; }
kernel void cpp_kernel(device float *out [[buffer(0)]]) { out[0] = 9.0f; }
kernel void cpp_const(device float *out [[buffer(0)]]) { out[0] = 10.0f; }
kernel void py_kernel(device float *out [[buffer(0)]]) { out[0] = 11.0f; }
kernel void py_const(device float *out [[buffer(0)]]) { out[0] = 12.0f; }
";

/// The same dispatch written in each other language a Metal host is: a name
/// passed at the lookup, and a name kept in a top-level constant that a
/// function reads.
const SWIFT_HOST: &str = "let kernelName = \"swift_const\"

func buildDirect(lib: MTLLibrary) {
    let f = lib.makeFunction(name: \"swift_kernel\")
}

func buildFromConst(lib: MTLLibrary) {
    let f = lib.makeFunction(name: kernelName)
}
";

const OBJC_HOST: &str = "static NSString *const kName = @\"objc_const\";

void buildDirect(id lib) {
    id f = [lib newFunctionWithName:@\"objc_kernel\"];
}

void buildFromConst(id lib) {
    id f = [lib newFunctionWithName:kName];
}
";

const CPP_HOST: &str = "static const char *kName = \"cpp_const\";

void buildDirect(MTL::Library *lib) {
    auto f = lib->newFunction(NS::String::string(\"cpp_kernel\", NS::UTF8StringEncoding));
}

void buildFromConst(MTL::Library *lib) {
    auto f = lib->newFunction(NS::String::string(kName, NS::UTF8StringEncoding));
}
";

const PY_HOST: &str = "KERNEL = \"py_const\"

def build_direct(lib):
    return lib.newFunctionWithName_(\"py_kernel\")

def build_from_const(lib):
    return lib.newFunctionWithName_(KERNEL)
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
        extract_file("App/Renderer.swift", SWIFT_HOST),
        extract_file("App/Renderer.m", OBJC_HOST),
        extract_file("src/renderer.cpp", CPP_HOST),
        extract_file("tools/run.py", PY_HOST),
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
        vec![
            "src/host.rs::rows".to_string(),
            "src/host.rs::rows_for".to_string()
        ],
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

/// Swift, Objective-C, C++ and Python hosts reach their kernels the way a Rust
/// host does — at the lookup, and through a top-level constant.
#[test]
fn every_host_language_reaches_the_kernels_it_names() {
    let store = store();
    for (kernel, direct, via_const) in [
        (
            "swift",
            "App/Renderer.swift::buildDirect",
            "App/Renderer.swift::buildFromConst",
        ),
        (
            "objc",
            "App/Renderer.m::buildDirect",
            "App/Renderer.m::buildFromConst",
        ),
        (
            "cpp",
            "src/renderer.cpp::buildDirect",
            "src/renderer.cpp::buildFromConst",
        ),
        (
            "py",
            "tools/run.py::build_direct",
            "tools/run.py::build_from_const",
        ),
    ] {
        let reached_direct = reached(
            &store,
            &format!("kernels/tune/tune.metal::{kernel}_kernel"),
            1,
        );
        assert_eq!(reached_direct, vec![direct.to_string()], "{kernel} direct");
        let reached_const = reached(
            &store,
            &format!("kernels/tune/tune.metal::{kernel}_const"),
            2,
        );
        assert!(
            reached_const.contains(&via_const.to_string()),
            "{kernel} const must reach {via_const}: {reached_const:?}"
        );
    }
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

/// Every kernel-name edge a fresh extraction yields survives the reduction the
/// store keeps — what an incremental build and the daemon resolve unchanged
/// files from. The kernel index reads stored wiring and symbols, and the host
/// side reads stored `EntryName` references with their constant bindings.
#[test]
fn kernel_name_edges_survive_the_stored_form_of_every_file() {
    fn kernel_edges(extractions: &[Extraction]) -> Vec<(String, String)> {
        let mut resolver = Resolver::new();
        resolver.index_extractions(extractions);
        let mut edges: Vec<(String, String)> = resolver
            .resolve_all(extractions)
            .unwrap()
            .edges
            .into_iter()
            .filter(|edge| {
                edge.target_file.ends_with(".metal") && !edge.source_file.ends_with(".metal")
            })
            .map(|edge| (edge.source_symbol, edge.target_symbol))
            .collect();
        edges.sort();
        edges.dedup();
        edges
    }
    let fresh = extractions();
    let stored: Vec<Extraction> = fresh.iter().map(Extraction::for_durable_store).collect();
    let expected = kernel_edges(&fresh);
    assert!(expected.len() >= 15, "{expected:?}");
    assert_eq!(kernel_edges(&stored), expected);
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
    // `src/renderer.cpp` is read by the same grammar and keeps its own row.
    let cpp = rate
        .by_language
        .get("cpp")
        .unwrap_or_else(|| panic!("no cpp row: {:?}", rate.by_language.keys()));
    let metal_files = extractions
        .iter()
        .filter(|ext| ext.file_path.ends_with(".metal"))
        .count();
    assert_eq!(metal_files, 2);
    assert!(
        cpp.resolved_sites + cpp.unresolved_sites < metal.resolved_sites + metal.unresolved_sites,
        "the Metal sites are not folded into the C++ row: cpp {cpp:?}, metal {metal:?}"
    );
}
