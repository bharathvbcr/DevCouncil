//! The page cache and file mapping of a read-only connection are a named,
//! asked-for setting, read back from SQLite.

use devmap_store::{ReaderMemory, Store};

fn store_file(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-reader-memory-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("devmap.sqlite");
    drop(Store::open(&file).expect("create store"));
    file
}

#[test]
fn a_plain_read_only_open_keeps_the_64_mib_cache_and_no_mapping() {
    let file = store_file("default");
    let store = Store::open_read_only(&file).expect("read-only open");
    assert_eq!(
        store.memory_settings().unwrap(),
        ReaderMemory {
            cache_kib: 65_536,
            mmap_bytes: 0
        }
    );
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}

#[test]
fn open_read_only_with_applies_the_asked_for_cache_and_mapping() {
    let file = store_file("with");
    let small = ReaderMemory {
        cache_kib: 16_384,
        mmap_bytes: 0,
    };
    let store = Store::open_read_only_with(&file, small).expect("read-only open");
    assert_eq!(store.memory_settings().unwrap(), small);

    let mapped = ReaderMemory {
        cache_kib: 16_384,
        mmap_bytes: 1 << 20,
    };
    let store = Store::open_read_only_with(&file, mapped).expect("read-only open");
    assert_eq!(store.memory_settings().unwrap(), mapped);
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}
