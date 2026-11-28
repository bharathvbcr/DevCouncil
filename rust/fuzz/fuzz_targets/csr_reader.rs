#![no_main]

use devmap_store::csr::{CsrGraphReader, CsrGraphWriter, CsrNodeRecord};
use libfuzzer_sys::fuzz_target;

/// Exercise every read accessor on an accepted `CsrGraphReader` so ASan/UBSan
/// observes any out-of-bounds slice, misaligned read, or length mismatch.
fn exercise_reader(reader: &CsrGraphReader) {
    assert!(
        reader.ensure_unchanged().is_ok(),
        "in-memory CSR reader must always report unchanged"
    );

    let node_count = reader.node_count();
    let edge_count = reader.edge_count();
    let _ = reader.generation();

    let scan_limit = node_count.min(4_096);
    let mut observed_edges = 0usize;
    for idx in 0..scan_limit {
        let u = idx as u32;
        assert!(
            reader.node(u).is_some(),
            "node({u}) returned None for idx < node_count ({node_count})"
        );
        let outgoing = reader.outgoing_edges(u);
        let attrs = reader.edge_attributes(u);
        assert_eq!(
            outgoing.len(),
            attrs.len(),
            "outgoing_edges and edge_attributes length mismatch at node {u}"
        );
        assert!(
            outgoing.len() <= edge_count,
            "node {u} outgoing degree {} exceeds total edge_count {edge_count}",
            outgoing.len()
        );
        observed_edges = observed_edges.saturating_add(outgoing.len());
    }
    assert!(
        observed_edges <= edge_count,
        "sum of scanned outgoing edges ({observed_edges}) exceeds edge_count ({edge_count})"
    );

    // Out-of-bounds queries must fail closed without panicking or reading OOB.
    if let Ok(oob) = u32::try_from(node_count) {
        assert!(reader.node(oob).is_none());
        assert!(reader.outgoing_edges(oob).is_empty());
        assert!(reader.edge_attributes(oob).is_empty());
    }
    assert!(reader.node(u32::MAX).is_none());
    assert!(reader.outgoing_edges(u32::MAX).is_empty());
    assert!(reader.edge_attributes(u32::MAX).is_empty());

    // Bounded traversal exercises multi-hop pointer chasing safely.
    if node_count > 0 {
        let _ = reader.bfs_reachability(0, 4);
    }
}

/// Build a valid CSR buffer from structured bytes, verify round-trip fidelity,
/// then mutate header/table bytes to reach deeper offset/alignment checks.
fn fuzz_structured_then_mutated(data: &[u8]) {
    if data.len() < 4 {
        return;
    }
    let generation = u64::from(data[0]);
    let node_count = usize::from(data[1] % 24);
    let edge_budget = usize::from(data[2] % 48);

    let mut writer = CsrGraphWriter::new(generation);
    for i in 0..node_count {
        writer.add_node(CsrNodeRecord {
            node_id: i as u32,
            kind: u16::from(data[0]),
            flags: u16::from(data[1] & 0x0F),
            out_degree: 0,
            _reserved: 0,
            name_hash: u64::from(i as u32).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            file_path_hash: u64::from(data[2]),
        });
    }

    let mut cursor = 3usize;
    let mut expected_edges = 0usize;
    if node_count > 0 {
        for _ in 0..edge_budget {
            if cursor + 3 > data.len() {
                break;
            }
            let src = u32::from(data[cursor]) % (node_count as u32);
            let dst = u32::from(data[cursor + 1]) % (node_count as u32);
            let attr = u16::from(data[cursor + 2]);
            cursor += 3;
            writer.add_edge(src, dst, attr);
            expected_edges += 1;
        }
    }

    let valid_bytes = writer.build();
    let reader = CsrGraphReader::from_bytes(valid_bytes.clone())
        .expect("CsrGraphWriter::build output must always parse cleanly");
    assert_eq!(reader.generation(), generation);
    assert_eq!(reader.node_count(), node_count);
    assert_eq!(reader.edge_count(), expected_edges);
    exercise_reader(&reader);

    // Apply fuzz-guided byte mutations and truncations to the valid CSR image.
    let mut mutated = valid_bytes;
    while cursor + 2 <= data.len() && !mutated.is_empty() {
        let op = data[cursor];
        let arg = data[cursor + 1];
        cursor += 2;
        match op & 0x03 {
            0 => {
                // Single-byte overwrite anywhere in the header or tables.
                let pos = usize::from(op >> 2) % mutated.len();
                mutated[pos] = arg;
            }
            1 => {
                // Targeted header-field corruption (bytes 0..80).
                let pos = usize::from(op >> 2) % mutated.len().min(80);
                mutated[pos] ^= arg.max(1);
            }
            2 => {
                // Truncate at arbitrary byte boundary (misaligned / short tail).
                let new_len = usize::from(arg) % mutated.len();
                mutated.truncate(new_len);
            }
            _ => {
                // Append unaligned trailing bytes.
                let extra = usize::from(arg & 0x0F);
                mutated.extend(std::iter::repeat_n(op, extra));
            }
        }
        if let Ok(mutated_reader) = CsrGraphReader::from_bytes(mutated.clone()) {
            exercise_reader(&mutated_reader);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // 1. Raw arbitrary bytes straight into the untrusted parser.
    if let Ok(reader) = CsrGraphReader::from_bytes(data.to_vec()) {
        exercise_reader(&reader);
    }

    // 2. Structured valid-graph round-trip + targeted header/table mutation.
    fuzz_structured_then_mutated(data);
});
