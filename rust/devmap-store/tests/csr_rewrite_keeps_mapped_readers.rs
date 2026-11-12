#![cfg(unix)]

//! The writer replaces a `.csr` file by rename, so a reader that mapped the
//! file before the rewrite keeps its old, complete image. Written in place
//! (`File::create` on the same name) the rewrite truncates the pages under the
//! reader, which is `SIGBUS` on its next touch.

use std::os::unix::fs::MetadataExt;

use devmap_store::csr::{CsrGraphReader, CsrGraphWriter, CsrNodeRecord};

/// A chain of `nodes` nodes, each pointing at the next.
fn chain(generation: u64, nodes: u32) -> CsrGraphWriter {
    let mut writer = CsrGraphWriter::new(generation);
    for _ in 0..nodes {
        writer.add_node(CsrNodeRecord::default());
    }
    for node in 0..nodes - 1 {
        writer.add_edge(node, node + 1, 90);
    }
    writer
}

#[test]
fn a_reader_mapped_before_a_rewrite_keeps_its_old_complete_image() {
    let dir = std::env::temp_dir().join(format!("devmap-csr-rename-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("graph.csr");
    chain(1, 2000).write_to_file(&path).unwrap();
    let old = CsrGraphReader::open_mmap(&path).unwrap();
    assert_eq!(old.node_count(), 2000);

    // A much smaller graph over the same name.
    let inode_before = std::fs::metadata(&path).unwrap().ino();
    chain(2, 3).write_to_file(&path).unwrap();
    assert_ne!(
        std::fs::metadata(&path).unwrap().ino(),
        inode_before,
        "the rewrite must replace the file, not write into the one that is mapped \
(on some hosts a truncated mapping keeps serving pages it already holds, so the \
reads below cannot tell the two apart)"
    );

    assert_eq!(old.generation(), 1);
    assert_eq!(old.node_count(), 2000);
    // Touch the last page of the old image.
    assert_eq!(old.outgoing_edges(1998), &[1999]);

    let new = CsrGraphReader::open_mmap(&path).unwrap();
    assert_eq!(new.generation(), 2);
    assert_eq!(new.node_count(), 3);
    assert!(
        std::fs::read_dir(&dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")),
        "no temporary file is left behind"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
