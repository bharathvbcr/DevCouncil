#![cfg(unix)]

//! `write_latest_csr_sidecar_strict` reports what the best-effort wrapper
//! absorbs.
//!
//! A standalone seed or a seeded build places the CSR sidecar next to the
//! store. If that copy fails silently the store is placed beside no sidecar, or
//! a stale one, and a reader maps the wrong graph. The strict writer therefore
//! returns every filesystem failure; the wrapper (read-time caching, where an
//! unwritable directory is not fatal) keeps returning `Ok(None)` for those, and
//! still returns an error when the generation's edges cannot be read at all.
//!
//! Every store here holds a real generation, written the way a build writes it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use devmap_analyze::analyze;
use devmap_extract::extract_tree_with_report;
use devmap_resolve::Resolver;
use devmap_store::csr::CsrGraphReader;
use devmap_store::{GenerationWriteOpts, Store};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "devmap-strict-csr-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An on-disk store at `<scratch>/devmap.sqlite` holding one real generation.
fn indexed_store(scratch: &Scratch) -> (Store, PathBuf, u32) {
    let tree = scratch.path().join("tree");
    fs::create_dir_all(tree.join("src")).unwrap();
    fs::write(
        tree.join("src/a.py"),
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n",
    )
    .unwrap();
    let (files, _report) = extract_tree_with_report(&tree).unwrap();
    assert!(
        !files.is_empty(),
        "fixture precondition: a file was indexed"
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let analysis = analyze(&files, &resolution);
    let db = scratch.path().join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let generation = store
        .save_generation_with_metadata(
            &files,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(tree.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
            "cafebabe1234",
        )
        .unwrap();
    (store, db, generation)
}

fn csr_of(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".csr");
    PathBuf::from(name)
}

#[test]
fn strict_write_places_a_sidecar_stamped_with_the_latest_generation() {
    let scratch = Scratch::new("places");
    let (store, db, generation) = indexed_store(&scratch);
    let written = store
        .write_latest_csr_sidecar_strict()
        .unwrap()
        .expect("a store with a generation has a sidecar to write");
    assert_eq!(written, csr_of(&db));
    let reader = CsrGraphReader::open_mmap(&written).unwrap();
    assert_eq!(reader.generation(), u64::from(generation));
}

#[test]
fn strict_write_surfaces_a_blocked_placement_the_wrapper_absorbs() {
    let scratch = Scratch::new("blocked");
    let (store, db, _generation) = indexed_store(&scratch);
    // A directory at the sidecar path: the temp file is written, the rename onto
    // a directory fails.
    fs::create_dir(csr_of(&db)).unwrap();

    let error = store
        .write_latest_csr_sidecar_strict()
        .expect_err("a placement that failed is an error, not an absent sidecar");
    assert!(
        format!("{error:#}").contains("cannot place CSR sidecar"),
        "the error names the step that failed: {error:#}"
    );
    assert_eq!(
        store.write_latest_csr_sidecar().unwrap(),
        None,
        "the read-time wrapper treats an unplaceable sidecar as absent"
    );
    let leftovers: Vec<_> = fs::read_dir(scratch.path())
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "a failed placement must not leave its temp file behind: {leftovers:?}"
    );
}

#[test]
fn strict_write_surfaces_an_unwritable_directory() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("unwritable");
    let (store, db, _generation) = indexed_store(&scratch);
    let dir = db.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = fs::write(dir.join("probe"), b"x");
    let outcome = store.write_latest_csr_sidecar_strict();
    let wrapper = store.write_latest_csr_sidecar();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if probe.is_ok() {
        // Running as a user the mode does not bind (root): nothing to assert.
        return;
    }
    let error = outcome.expect_err("an unwritable directory is an error");
    assert!(
        format!("{error:#}").contains("CSR sidecar"),
        "the error names the sidecar: {error:#}"
    );
    assert_eq!(wrapper.unwrap(), None);
}

/// A directory that can be written and entered but not opened for reading:
/// the sidecar is created and renamed into place, then syncing the directory
/// fails. The sidecar is there, and the error must not say it is not.
#[test]
fn strict_write_says_written_when_only_the_directory_sync_failed() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("unsynced");
    let (store, db, generation) = indexed_store(&scratch);
    let dir = db.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o300)).unwrap();
    let readable = fs::read_dir(&dir).is_ok();
    let outcome = store.write_latest_csr_sidecar_strict();
    let wrapper = store.write_latest_csr_sidecar();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        // Running as a user the mode does not bind (root): nothing to assert.
        return;
    }

    let error = outcome.expect_err("the directory sync failed, which is reported");
    let message = format!("{error:#}");
    assert!(
        message.contains("was written") && !message.contains("cannot write"),
        "a sidecar that is in place is not reported as not written: {message}"
    );
    let sidecar = csr_of(&db);
    assert_eq!(
        CsrGraphReader::open_mmap(&sidecar).unwrap().generation(),
        u64::from(generation),
        "the sidecar is whole and current"
    );
    assert_eq!(
        wrapper.unwrap(),
        Some(sidecar),
        "the read-time wrapper has the file it was asked to have"
    );
}

#[test]
fn strict_write_has_nothing_to_write_for_a_store_without_a_generation_or_without_a_file() {
    let scratch = Scratch::new("empty");
    let empty = Store::open(scratch.path().join("empty.sqlite")).unwrap();
    assert_eq!(empty.write_latest_csr_sidecar_strict().unwrap(), None);
    assert!(!csr_of(&scratch.path().join("empty.sqlite")).exists());

    let in_memory = Store::open_in_memory().unwrap();
    assert_eq!(in_memory.write_latest_csr_sidecar_strict().unwrap(), None);
}
