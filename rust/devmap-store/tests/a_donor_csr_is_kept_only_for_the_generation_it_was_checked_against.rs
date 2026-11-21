//! `copy_donor_csr_checked`: the donor's CSR is copied first and the copy is
//! what is checked, then kept only when stamped with the expected generation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use devmap_store::csr::{CsrGraphWriter, CsrNodeRecord};
use devmap_store::seed::{copy_donor_csr_checked, csr_generation_of_open_file, DonorCsr};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-donor-csr-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_csr(path: &Path, generation: u64) {
    let mut writer = CsrGraphWriter::new(generation);
    writer.add_node(CsrNodeRecord::default());
    writer.write_to_file(path).unwrap();
}

#[test]
fn a_donor_csr_of_the_expected_generation_is_kept() {
    let dir = tmp_dir("kept");
    let (donor, staged) = (dir.join("donor.csr"), dir.join("staged.csr"));
    write_csr(&donor, 4);
    assert_eq!(
        copy_donor_csr_checked(&donor, &staged, 4).unwrap(),
        DonorCsr::Kept
    );
    assert!(staged.is_file());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_donor_csr_of_another_generation_is_removed_from_the_staged_path() {
    let dir = tmp_dir("other");
    let (donor, staged) = (dir.join("donor.csr"), dir.join("staged.csr"));
    write_csr(&donor, 9);
    let outcome = copy_donor_csr_checked(&donor, &staged, 4).unwrap();
    assert!(matches!(outcome, DonorCsr::Rejected { .. }), "{outcome:?}");
    assert!(
        !staged.exists(),
        "a rejected copy must not stay at the staged path"
    );
    assert!(donor.is_file(), "the donor's own file is never touched");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_unreadable_donor_csr_is_rejected_not_an_error() {
    let dir = tmp_dir("garbage");
    let (donor, staged) = (dir.join("donor.csr"), dir.join("staged.csr"));
    fs::write(&donor, b"not a csr").unwrap();
    let outcome = copy_donor_csr_checked(&donor, &staged, 4).unwrap();
    assert!(matches!(outcome, DonorCsr::Rejected { .. }), "{outcome:?}");
    assert!(!staged.exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_donor_without_a_csr_places_nothing() {
    let dir = tmp_dir("absent");
    let staged = dir.join("staged.csr");
    let outcome = copy_donor_csr_checked(&dir.join("missing.csr"), &staged, 4).unwrap();
    assert_eq!(outcome, DonorCsr::Absent);
    assert!(!staged.exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_generation_is_read_through_the_open_descriptor() {
    let dir = tmp_dir("open-file");
    let path = dir.join("g.csr");
    write_csr(&path, 17);
    let file = fs::File::open(&path).unwrap();
    assert_eq!(csr_generation_of_open_file(&file).unwrap(), 17);
    let _ = fs::remove_dir_all(dir);
}
