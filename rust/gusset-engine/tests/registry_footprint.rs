//! The registry's byte bound is a bound on memory, not on pattern text.
//!
//! One test in its own binary, so the process-wide allocator count it reads
//! moves only for what this test does. The umbrella crate declares Gusset's
//! counting allocator as the global one, which is what `get_alloc_stats`
//! reports.

use gusset::{prepare_frame, Registry, MAX_REGISTRY_BYTES, REGISTRY_FULL};
use gusset_core::get_alloc_stats;

/// dc-glob's own cap, in characters: a longer pattern is not compiled at all.
const PATTERN_UNITS: usize = 16_384;

fn prepare(registry: &Registry, patterns: &[Vec<u8>]) -> u32 {
    let mut frame = Vec::new();
    frame.extend_from_slice(&(patterns.len() as u32).to_le_bytes());
    for pattern in patterns {
        frame.extend_from_slice(&(pattern.len() as u32).to_le_bytes());
        frame.extend_from_slice(pattern);
    }
    let id = prepare_frame(registry, &frame).expect("a well-formed frame");
    u32::from_le_bytes(id.try_into().expect("an id is four bytes"))
}

/// The worst case per byte of frame: patterns of plain characters at
/// dc-glob's cap, one token each. Charged at six bytes per frame byte, a
/// registry filled to its bound held roughly ten times it.
#[test]
fn a_registry_filled_to_its_bound_holds_about_that_much_memory() {
    let registry = Registry::new();
    let before = get_alloc_stats().live_bytes;
    let mut lists = 0usize;
    loop {
        let patterns: Vec<Vec<u8>> = (0..4)
            .map(|k| {
                let mut p = format!("{lists}-{k}-").into_bytes();
                p.resize(PATTERN_UNITS, b'x');
                p
            })
            .collect();
        if prepare(&registry, &patterns) == REGISTRY_FULL {
            break;
        }
        lists += 1;
        assert!(lists < 4096, "the byte bound never filled the registry");
    }
    let held = get_alloc_stats().live_bytes.saturating_sub(before);
    assert!(lists > 0, "not one list fit under the bound");
    assert!(
        held <= MAX_REGISTRY_BYTES + MAX_REGISTRY_BYTES / 2,
        "{lists} lists hold {held} bytes against a bound of {MAX_REGISTRY_BYTES}"
    );
    // And the bound is spent, not left mostly idle by an overcharge.
    assert!(
        held >= MAX_REGISTRY_BYTES / 2,
        "{lists} lists hold only {held} bytes of a {MAX_REGISTRY_BYTES} bound"
    );
    drop(registry);
}
