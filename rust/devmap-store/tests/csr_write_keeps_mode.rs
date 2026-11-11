#![cfg(unix)]

//! Replacing a `.csr` file by rename must not change who can read it, and must
//! leave nothing but the target behind.

use std::os::unix::fs::PermissionsExt;

use devmap_store::csr::{CsrGraphReader, CsrGraphWriter, CsrNodeRecord};

fn graph(generation: u64) -> CsrGraphWriter {
    let mut writer = CsrGraphWriter::new(generation);
    writer.add_node(CsrNodeRecord::default());
    writer.add_node(CsrNodeRecord::default());
    writer.add_edge(0, 1, 90);
    writer
}

#[test]
fn a_replaced_file_keeps_its_mode_and_no_temp_file_is_left() {
    let dir = std::env::temp_dir().join(format!("devmap-csr-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("graph.csr");

    graph(1).write_to_file(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

    graph(2).write_to_file(&path).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o640,
        "the replacement must keep the replaced file's mode"
    );
    assert_eq!(CsrGraphReader::open_mmap(&path).unwrap().generation(), 2);
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["graph.csr".to_string()], "no temp file left");
    std::fs::remove_dir_all(dir).unwrap();
}
