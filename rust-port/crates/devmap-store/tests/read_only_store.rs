//! A store the process cannot write is still a store it can read.
//!
//! A read-only store is an ordinary situation, not a corrupt one: an index
//! shipped in a CI cache, a shared checkout mounted read-only, a `chmod 444`
//! a cautious operator applied so a hook could never rebuild under them. On
//! 2026-09-06 `devmap status` and every query refused such a store outright
//! with SQLite's "attempt to write a readonly database" — the *open* path, not
//! the query, was writing. Nothing a read asks for needs that write, so the
//! refusal turned a readable map into no map at all.
//!
//! The contract these tests pin: opening a current-schema store that is not
//! writable succeeds; `status`, symbol search and the generation listing
//! answer from it; and a write against it fails with a reason that names the
//! store as read-only rather than a raw SQLite code.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_resolve::Resolver;
use devmap_store::Store;

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-read-only-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// One generation with one symbol, written through the public store API, so
/// the read-only store under test is a store this kernel wrote at the current
/// schema — the shape an operator actually locks down.
fn write_one_generation(db: &Path) {
    let store = Store::open(db).expect("a fresh store opens writable");
    let extraction = extract_file("lib.py", "def keep():\n    return 1\n");
    let extractions = vec![extraction];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("a generation persists to a writable store");
    // Drop the store so its WAL is checkpointed back into the main file and
    // no `-wal`/`-shm` sidecar is left holding un-checkpointed frames: the
    // read-only case must be about the database, not about a sidecar the
    // writer left behind.
    drop(store);
    let _ = fs::remove_file(db.with_extension("sqlite-wal"));
    let _ = fs::remove_file(db.with_extension("sqlite-shm"));
}

fn set_mode(path: &Path, mode: u32) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(mode);
        let _ = fs::set_permissions(path, permissions);
    }
}

/// The file itself is read-only; its directory is not. This is the `chmod
/// 444` an operator applies, and it is the case the CLI refused.
#[test]
fn a_read_only_store_file_still_answers_status_and_search() {
    let dir = scratch_dir("file");
    let db = dir.join("devmap.sqlite");
    write_one_generation(&db);
    set_mode(&db, 0o444);

    let outcome = (|| -> anyhow::Result<(Option<u32>, usize, Option<u32>)> {
        let store = Store::open(&db)?;
        let status = store.status(&db.display().to_string())?;
        let hits = store.search_symbols("keep", 10)?;
        let latest = store.latest_generation_id()?;
        Ok((status.latest_generation, hits.len(), latest))
    })();

    set_mode(&db, 0o644);
    let _ = fs::remove_dir_all(&dir);

    let (generation, hits, latest) = match outcome {
        Ok(answer) => answer,
        Err(error) => panic!("a read-only store must open and answer reads, got: {error:#}"),
    };
    assert_eq!(
        generation,
        Some(1),
        "status must report the generation the store holds"
    );
    assert_eq!(hits, 1, "the one persisted symbol must be searchable");
    assert_eq!(latest, Some(1));
}

/// The directory is read-only too, so SQLite cannot even create the `-shm`
/// and `-wal` sidecars a WAL database normally wants. Reads must still work:
/// SQLite supports this shape explicitly (a read-only open of a WAL database
/// whose sidecars are absent), and a store on a read-only mount is exactly
/// this case.
#[test]
fn a_store_in_a_read_only_directory_still_answers_reads() {
    let dir = scratch_dir("dir");
    let db = dir.join("devmap.sqlite");
    write_one_generation(&db);
    set_mode(&db, 0o444);
    set_mode(&dir, 0o555);

    let outcome = (|| -> anyhow::Result<usize> {
        let store = Store::open(&db)?;
        Ok(store.search_symbols("keep", 10)?.len())
    })();

    set_mode(&dir, 0o755);
    set_mode(&db, 0o644);
    let _ = fs::remove_dir_all(&dir);

    match outcome {
        Ok(hits) => assert_eq!(hits, 1, "the persisted symbol must be readable"),
        Err(error) => {
            panic!("reads from a store in a read-only directory must work, got: {error:#}")
        }
    }
}

/// A write against a read-only store must fail — and say why in the store's
/// own words. "Error code 8" is a fact about SQLite; "read-only" is the fact
/// the operator can act on.
#[test]
fn a_write_to_a_read_only_store_is_refused_by_name() {
    let dir = scratch_dir("write");
    let db = dir.join("devmap.sqlite");
    write_one_generation(&db);
    set_mode(&db, 0o444);

    let outcome = (|| -> anyhow::Result<()> {
        let store = Store::open(&db)?;
        let extractions = vec![extract_file("lib.py", "def keep():\n    return 2\n")];
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = analyze(&extractions, &resolution);
        store.save_generation(&extractions, &resolution, &analysis)?;
        Ok(())
    })();

    set_mode(&db, 0o644);
    let _ = fs::remove_dir_all(&dir);

    let error = match outcome {
        Ok(()) => panic!("a generation must not persist into a read-only store"),
        Err(error) => format!("{error:#}"),
    };
    assert!(
        error.contains("read-only"),
        "the refusal must name the store as read-only, got: {error}"
    );
}

/// SQLite creates a store's `-wal` and `-shm` with the database file's mode.
/// So a read of a 444 store leaves 444 sidecars, and once the operator gives
/// the store its write bit back the sidecars still lack theirs — the next
/// build then fails against a store that is, by every check the operator
/// would make, writable. The kernel owns those sidecars, so it restores their
/// mode when the store itself is writable.
#[test]
fn a_store_made_writable_again_is_writable_despite_read_only_sidecars() {
    let dir = scratch_dir("sidecars");
    let db = dir.join("devmap.sqlite");
    write_one_generation(&db);
    set_mode(&db, 0o444);
    {
        // The read that leaves 444 sidecars behind.
        let store = Store::open(&db).expect("a read-only store opens");
        assert_eq!(store.search_symbols("keep", 10).expect("read").len(), 1);
    }
    let shm = PathBuf::from(format!("{}-shm", db.display()));
    assert!(
        shm.exists(),
        "precondition: the read-only open must have created the WAL index sidecar"
    );
    set_mode(&db, 0o644);

    let outcome = (|| -> anyhow::Result<u32> {
        let store = Store::open(&db)?;
        let extractions = vec![extract_file("lib.py", "def keep():\n    return 2\n")];
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = analyze(&extractions, &resolution);
        Ok(store.save_generation(&extractions, &resolution, &analysis)?)
    })();

    set_mode(&shm, 0o644);
    set_mode(&db, 0o644);
    let _ = fs::remove_dir_all(&dir);

    match outcome {
        Ok(generation) => assert_eq!(generation, 2, "the second generation must persist"),
        Err(error) => {
            panic!("a store given its write bit back must accept a write, got: {error:#}")
        }
    }
}
