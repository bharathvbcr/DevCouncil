//! The `.csr` reader trusts nothing it reads from the file.
//!
//! `CsrGraphReader` hands out slices built from offsets, counts and row
//! boundaries stored in the file. A corrupt, truncated or hostile file must be
//! refused when it is opened, never answered from: a count whose byte size
//! wraps `usize` or a row boundary past the edge array turns into a slice
//! outside the mapping, which is undefined behaviour and in practice a
//! segfault in a long-lived server.
//!
//! The on-disk format is read by GitPulse's vendored copy of this crate and is
//! not changed here; every case below is a file the writer never produces.

use devmap_store::csr::{CsrGraphReader, CsrGraphWriter, CsrNodeRecord};

/// Header field offsets (the `repr(C)` layout of `CsrHeader`).
const VERSION: usize = 8;
const NODE_COUNT: usize = 24;
const EDGE_COUNT: usize = 32;
const NODES_OFFSET: usize = 40;
const ROW_OFFSETS_OFFSET: usize = 48;
const COL_INDICES_OFFSET: usize = 56;
const ATTRIBUTES_OFFSET: usize = 64;

/// A small valid graph: 0 -> 1 -> 2, plus 0 -> 2.
fn valid() -> Vec<u8> {
    let mut writer = CsrGraphWriter::new(7);
    for _ in 0..3 {
        writer.add_node(CsrNodeRecord::default());
    }
    writer.add_edge(0, 1, 90);
    writer.add_edge(0, 2, 80);
    writer.add_edge(1, 2, 70);
    writer.build()
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn refused(bytes: Vec<u8>, why: &str) {
    match CsrGraphReader::from_bytes(bytes) {
        Ok(_) => panic!("{why}: the reader accepted a file it cannot safely read"),
        Err(error) => assert!(!error.to_string().is_empty(), "{why}: an empty reason"),
    }
}

/// Everything a caller can do with an accepted reader, which must not panic or
/// read outside the file: every node, every row, every attribute row, a walk.
fn exercise(reader: &CsrGraphReader) {
    for node in 0..reader.node_count().min(10_000) as u32 {
        let _ = reader.node(node);
        assert_eq!(
            reader.outgoing_edges(node).len(),
            reader.edge_attributes(node).len()
        );
        assert!(reader.outgoing_edges(node).len() <= reader.edge_count());
    }
    let _ = reader.bfs_reachability(0, 4);
    let _ = reader.outgoing_edges(u32::MAX);
}

#[test]
fn a_valid_file_is_accepted_and_answers() {
    let reader = CsrGraphReader::from_bytes(valid()).expect("the writer's own output");
    assert_eq!(reader.outgoing_edges(0), &[1, 2]);
    assert_eq!(reader.edge_attributes(0), &[90, 80]);
    exercise(&reader);
}

#[test]
fn every_truncation_of_a_valid_file_is_refused() {
    let whole = valid();
    for len in 0..whole.len() {
        let prefix = whole[..len].to_vec();
        match CsrGraphReader::from_bytes(prefix) {
            Err(_) => {}
            // The writer pads the last section to 8 bytes; a cut inside that
            // padding loses nothing the reader reads and is still the same graph.
            Ok(reader) => {
                assert_eq!(reader.edge_count(), 3, "cut at {len}");
                exercise(&reader);
            }
        }
    }
}

#[test]
fn counts_whose_byte_sizes_wrap_the_address_space_are_refused() {
    // 2^61 nodes * 32 bytes, (2^61 + 1) * 8 and 2^63 edges * 4 and * 2 all wrap
    // to a few bytes in 64-bit arithmetic, so a reader that multiplies without
    // checking sees four sections that "fit" and a graph of 2^61 nodes.
    let mut bytes = valid();
    put_u64(&mut bytes, NODE_COUNT, 1 << 61);
    put_u64(&mut bytes, EDGE_COUNT, 1 << 63);
    refused(bytes, "wrapping counts");
}

#[test]
fn offsets_near_the_top_of_the_address_space_are_refused() {
    for field in [
        NODES_OFFSET,
        ROW_OFFSETS_OFFSET,
        COL_INDICES_OFFSET,
        ATTRIBUTES_OFFSET,
    ] {
        let mut bytes = valid();
        put_u64(&mut bytes, field, u64::MAX - 3);
        refused(bytes, "an offset that overflows when added to a length");
    }
}

#[test]
fn offsets_past_the_end_of_the_file_are_refused() {
    for field in [
        NODES_OFFSET,
        ROW_OFFSETS_OFFSET,
        COL_INDICES_OFFSET,
        ATTRIBUTES_OFFSET,
    ] {
        let mut bytes = valid();
        let past = bytes.len() as u64 + 8;
        put_u64(&mut bytes, field, past);
        refused(bytes, "an offset past the end");
    }
}

#[test]
fn misaligned_section_offsets_are_refused() {
    for field in [
        NODES_OFFSET,
        ROW_OFFSETS_OFFSET,
        COL_INDICES_OFFSET,
        ATTRIBUTES_OFFSET,
    ] {
        let mut bytes = valid();
        let at = get_u64(&bytes, field) + 1;
        put_u64(&mut bytes, field, at);
        refused(bytes, "an offset that breaks the element alignment");
    }
}

#[test]
fn a_section_that_overlaps_the_header_is_refused() {
    let mut bytes = valid();
    put_u64(&mut bytes, NODES_OFFSET, 0);
    refused(bytes, "nodes starting inside the header");
}

#[test]
fn row_boundaries_that_decrease_are_refused() {
    // Row i's edges are col_indices[row[i]..row[i + 1]]; a decreasing pair makes
    // the length negative, which as `usize` is a slice of 2^64 elements.
    let mut bytes = valid();
    let rows = get_u64(&bytes, ROW_OFFSETS_OFFSET) as usize;
    put_u64(&mut bytes, rows + 8, 2);
    put_u64(&mut bytes, rows + 16, 1);
    refused(bytes, "decreasing row boundaries");
}

#[test]
fn row_boundaries_past_the_edge_array_are_refused() {
    let mut bytes = valid();
    let rows = get_u64(&bytes, ROW_OFFSETS_OFFSET) as usize;
    put_u64(&mut bytes, rows + 8, 1_000_000);
    put_u64(&mut bytes, rows + 16, 2_000_000);
    refused(bytes, "row boundaries beyond the edge count");
}

#[test]
fn row_boundaries_that_do_not_start_at_zero_or_end_at_the_edge_count_are_refused() {
    let rows_at = |bytes: &[u8]| get_u64(bytes, ROW_OFFSETS_OFFSET) as usize;

    let mut bytes = valid();
    let rows = rows_at(&bytes);
    put_u64(&mut bytes, rows, 1);
    refused(bytes, "first boundary not zero");

    let mut bytes = valid();
    let rows = rows_at(&bytes);
    let last = rows + 3 * 8;
    put_u64(&mut bytes, last, 2);
    refused(bytes, "last boundary short of the edge count");
}

#[test]
fn an_unknown_version_or_magic_is_refused() {
    let mut bytes = valid();
    bytes[VERSION] = 9;
    refused(bytes, "unknown version");
    let mut bytes = valid();
    bytes[0] ^= 0xFF;
    refused(bytes, "bad magic");
}

#[test]
fn an_empty_or_header_sized_file_is_refused_on_disk_too() {
    let dir = std::env::temp_dir().join(format!("devmap-csr-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.csr");
    std::fs::write(&empty, b"").unwrap();
    assert!(CsrGraphReader::open_mmap(&empty).is_err());
    let short = dir.join("short.csr");
    std::fs::write(&short, &valid()[..40]).unwrap();
    assert!(CsrGraphReader::open_mmap(&short).is_err());
    let hostile = dir.join("hostile.csr");
    let mut bytes = valid();
    put_u64(&mut bytes, NODE_COUNT, 1 << 61);
    put_u64(&mut bytes, EDGE_COUNT, 1 << 63);
    std::fs::write(&hostile, &bytes).unwrap();
    assert!(CsrGraphReader::open_mmap(&hostile).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Deterministic mutation sweep: overwrite each header byte and a spread of
/// table bytes with several values. Whatever the reader decides, an accepted
/// file must be fully readable. Before the fix this aborts the test process.
#[test]
fn mutated_files_are_either_refused_or_fully_readable() {
    let whole = valid();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for position in 0..whole.len() {
        for value in [0x00, 0x01, 0x7F, 0x80, 0xFF, (next() & 0xFF) as u8] {
            let mut bytes = whole.clone();
            bytes[position] = value;
            if let Ok(reader) = CsrGraphReader::from_bytes(bytes) {
                exercise(&reader);
            }
        }
    }
    for _ in 0..4_000 {
        let mut bytes = whole.clone();
        for _ in 0..3 {
            let at = (next() as usize) % bytes.len();
            bytes[at] = (next() & 0xFF) as u8;
        }
        if let Ok(reader) = CsrGraphReader::from_bytes(bytes) {
            exercise(&reader);
        }
    }
}
