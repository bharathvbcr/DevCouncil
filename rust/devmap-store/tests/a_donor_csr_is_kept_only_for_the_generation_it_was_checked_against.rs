//! `copy_donor_csr_checked`: the donor's CSR is copied first and the copy is
//! what is checked, then kept only when stamped with the expected generation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use devmap_store::csr::{CsrGraphReader, CsrGraphWriter, CsrHeader, CsrNodeRecord};
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

/// The stamp is read from the header alone: a file cut right after the header
/// (whose sections a full parse would refuse as past the end) still names its
/// generation, and one too short for a header or with the wrong magic is an error.
#[test]
fn the_generation_is_read_from_the_header_without_reading_the_body() {
    let dir = tmp_dir("header-only");
    let path = dir.join("g.csr");
    write_csr(&path, 23);
    let bytes = fs::read(&path).unwrap();
    let header_len = std::mem::size_of::<CsrHeader>();
    assert!(bytes.len() > header_len, "premise: the file has a body");
    fs::write(&path, &bytes[..header_len]).unwrap();
    let file = fs::File::open(&path).unwrap();
    assert_eq!(csr_generation_of_open_file(&file).unwrap(), 23);

    fs::write(&path, &bytes[..header_len - 1]).unwrap();
    let short = fs::File::open(&path).unwrap();
    assert!(csr_generation_of_open_file(&short).is_err());

    let mut wrong = bytes.clone();
    wrong[0] ^= 0xff;
    fs::write(&path, &wrong).unwrap();
    let bad_magic = fs::File::open(&path).unwrap();
    assert!(csr_generation_of_open_file(&bad_magic).is_err());
    let _ = fs::remove_dir_all(dir);
}

/// Every 8-byte header word set to 2^63-1, u64::MAX and 0, and every truncation:
/// the check must answer (Kept, Rejected or an error), never panic or allocate
/// by the claimed size. Debug build, so an unchecked add or multiply panics.
#[test]
fn a_corrupted_or_truncated_donor_csr_is_rejected_without_panicking() {
    let dir = tmp_dir("fuzz-header");
    let valid = dir.join("valid.csr");
    write_csr(&valid, 4);
    let bytes = fs::read(&valid).unwrap();
    let mut cases: Vec<(String, Vec<u8>)> = Vec::new();
    for offset in (0..bytes.len().min(160)).step_by(8) {
        for word in [
            u64::MAX,
            (1u64 << 63) - 1,
            0,
            1 << 32,
            u64::from(u32::MAX) + 1,
        ] {
            let mut mutated = bytes.clone();
            let end = (offset + 8).min(mutated.len());
            mutated[offset..end].copy_from_slice(&word.to_le_bytes()[..end - offset]);
            cases.push((format!("word {word:#x} at {offset}"), mutated));
        }
    }
    for cut in 0..bytes.len() {
        cases.push((format!("truncated to {cut}"), bytes[..cut].to_vec()));
    }
    for (label, mutated) in &cases {
        let (donor, staged) = (dir.join("donor.csr"), dir.join("staged.csr"));
        let _ = fs::remove_file(&staged);
        fs::write(&donor, mutated).unwrap();
        let outcome = std::panic::catch_unwind(|| {
            let kept = copy_donor_csr_checked(&donor, &staged, 4);
            let file = fs::File::open(&donor).unwrap();
            let _ = csr_generation_of_open_file(&file);
            kept
        });
        match outcome {
            Err(_) => panic!("{label}: the check panicked"),
            Ok(Ok(DonorCsr::Kept)) => {
                // Kept is only right when the bytes still read as generation 4.
                let file = fs::File::open(&staged).unwrap();
                assert_eq!(csr_generation_of_open_file(&file).unwrap(), 4, "{label}");
                // The header alone names the generation; a kept copy must
                // also be a CSR a reader can open.
                assert!(
                    CsrGraphReader::open_mmap(&staged).is_ok(),
                    "{label}: kept a copy no reader can open"
                );
            }
            Ok(_) => assert!(!staged.exists(), "{label}: a rejected copy stayed"),
        }
    }
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn a_donor_csr_that_is_a_symlink_a_directory_or_a_fifo_places_nothing() {
    let dir = tmp_dir("donor-kinds");
    let real = dir.join("real.csr");
    write_csr(&real, 4);
    let link = dir.join("link.csr");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let directory = dir.join("dir.csr");
    fs::create_dir(&directory).unwrap();
    let fifo = dir.join("fifo.csr");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    for donor in [&link, &directory, &fifo] {
        let staged = dir.join("staged.csr");
        let outcome = copy_donor_csr_checked(donor, &staged, 4).unwrap();
        assert_eq!(outcome, DonorCsr::Absent, "{}", donor.display());
        assert!(!staged.exists(), "{}", donor.display());
    }
    let _ = fs::remove_dir_all(dir);
}
