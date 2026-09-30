//! Adversarial input for the Python path-load reader (`pyload`).
//!
//! The reader runs on every Python file that names a loader API. A path
//! expression is a left-nested tree one level per `/`, so a recursive reader
//! overflows the stack on a long one; a literal can be arbitrarily large; and
//! a file can hold thousands of loads. Each case below is chosen to break one
//! of those, in the manner of `wiring_port_under_stress.rs`.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use std::time::{Duration, Instant};

fn path_load_count(extraction: &Extraction) -> usize {
    extraction
        .imports
        .iter()
        .filter(|import| import.path_load.is_some())
        .count()
}

/// Run on a thread with a small stack, so a recursive walk of a deep path
/// fails here instead of passing on a generous main-thread stack.
fn extract_on_small_stack(path: &'static str, source: String) -> Extraction {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || extract_file(path, &source))
        .expect("spawn")
        .join()
        .expect("extraction must not overflow the stack")
}

/// A 10,000-segment `/` chain: refused past the segment cap, and bounded.
#[test]
fn a_ten_thousand_segment_path_is_refused_without_overflow() {
    let chain: String = (0..10_000).map(|i| format!(" / \"d{i}\"")).collect();
    let source = format!(
        "import importlib.util\nfrom pathlib import Path\n\
         spec = importlib.util.spec_from_file_location(\"m\", Path(__file__).parent{chain} / \"x.py\")\n\
         mod = importlib.util.module_from_spec(spec)\n"
    );
    let start = Instant::now();
    let extraction = extract_on_small_stack("pkg/deep.py", source);
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "took {:?}",
        start.elapsed()
    );
    assert_eq!(
        path_load_count(&extraction),
        0,
        "a path past the segment cap must abstain, not be read"
    );
}

/// A path just under the cap is still read — the cap bounds, it does not
/// disable.
#[test]
fn a_path_under_the_segment_cap_is_still_read() {
    let chain: String = (0..100).map(|i| format!(" / \"d{i}\"")).collect();
    let source = format!(
        "import importlib.util\nfrom pathlib import Path\n\
         spec = importlib.util.spec_from_file_location(\"m\", Path(__file__).parent{chain} / \"x.py\")\n\
         mod = importlib.util.module_from_spec(spec)\n"
    );
    let extraction = extract_on_small_stack("pkg/deep.py", source);
    assert_eq!(path_load_count(&extraction), 1, "{:?}", extraction.imports);
}

/// A 100,000-character literal is refused, not copied into the payload.
#[test]
fn a_hundred_thousand_character_literal_is_refused() {
    let huge = "a".repeat(100_000);
    let source = format!(
        "import importlib.util\n\
         spec = importlib.util.spec_from_file_location(\"m\", \"{huge}.py\")\n\
         mod = importlib.util.module_from_spec(spec)\n"
    );
    let extraction = extract_file("pkg/huge.py", &source);
    assert_eq!(path_load_count(&extraction), 0);
    let payload = serde_json::to_string(&extraction.imports).unwrap();
    assert!(
        payload.len() < 10_000,
        "the literal leaked into the imports payload: {} bytes",
        payload.len()
    );
}

/// Deeply nested wrappers — `str(str(str(...)))` — are bounded by depth.
#[test]
fn deeply_nested_wrappers_are_bounded() {
    let depth = 5_000;
    let open = "str(".repeat(depth);
    let close = ")".repeat(depth);
    let source = format!(
        "import importlib.util\n\
         spec = importlib.util.spec_from_file_location(\"m\", {open}\"x.py\"{close})\n\
         mod = importlib.util.module_from_spec(spec)\n"
    );
    let extraction = extract_on_small_stack("pkg/nested.py", source);
    assert_eq!(path_load_count(&extraction), 0);
}

/// Thousands of loads in one file are all read, through the real extractor.
///
/// Correctness only. The *cost* of the pass is measured in isolation by
/// `pyload::tests::thousands_of_loads_cost_linear_time`: timed through
/// `extract_file`, the rest of the extractor dominates — on a 3,000-line
/// file without a single load it is itself superlinear in a debug build — so
/// a ratio taken here would judge that, not this pass.
#[test]
fn thousands_of_loads_in_one_file_are_all_read() {
    let count = 1_000;
    let mut source = String::from("import imp\n");
    for i in 0..count {
        source.push_str(&format!(
            "m{i} = imp.load_source(\"m{i}\", \"lib/m{i}.py\")\n"
        ));
    }
    let extraction = extract_file("lib/many.py", &source);
    assert!(
        !matches!(
            extraction.parse_outcome,
            devmap_extract::model::ParseOutcome::Failed { .. }
        ),
        "{:?}",
        extraction.parse_outcome
    );
    assert_eq!(path_load_count(&extraction), count);
}
