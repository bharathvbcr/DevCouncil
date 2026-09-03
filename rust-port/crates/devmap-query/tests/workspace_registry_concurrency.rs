//! Registering repositories concurrently must not lose any of them.
//!
//! `devmap workspace add` was an unlocked read-modify-write over a file written
//! through a *shared* temp name — `workspace.json.tmp`, the exact collision
//! `artifacts.rs::write_atomic` documents as fixed for `repo_map.tmp`. Two
//! writers therefore had two ways to lose: the second `rename` could fail with
//! ENOENT after the first moved the shared temp away, and two writers that both
//! read the registry before either wrote it each saved their own entry over the
//! other's.

use devmap_query::workspace::Workspace;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-ws-rmw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

const WRITERS: usize = 8;

#[test]
fn eight_concurrent_registrations_all_survive() {
    let dir = scratch("add");
    let root = Arc::new(dir.clone());

    let handles: Vec<_> = (0..WRITERS)
        .map(|writer| {
            let root = Arc::clone(&root);
            thread::spawn(move || {
                Workspace::update(root.as_path(), |workspace| {
                    workspace.add(
                        format!("repo{writer}"),
                        PathBuf::from(format!("/tmp/repo{writer}")),
                    );
                })
                .map(|_| ())
            })
        })
        .collect();

    for (writer, handle) in handles.into_iter().enumerate() {
        let outcome = handle.join().expect("writer panicked");
        assert!(
            outcome.is_ok(),
            "writer {writer} failed: {:?}",
            outcome.err()
        );
    }

    let registry = Workspace::load(&dir).expect("registry must be readable");
    let names: Vec<&str> = registry
        .repos
        .iter()
        .map(|repo| repo.name.as_str())
        .collect();
    assert_eq!(
        registry.repos.len(),
        WRITERS,
        "concurrent registrations lost entries: {names:?}"
    );
    for writer in 0..WRITERS {
        let wanted = format!("repo{writer}");
        assert!(
            names.contains(&wanted.as_str()),
            "{wanted} was registered but is not in the registry: {names:?}"
        );
    }

    // A temp file left behind is what the next writer trips over.
    let strays: Vec<String> = std::fs::read_dir(dir.join(".devcouncil"))
        .expect("registry directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(strays.is_empty(), "temp files left behind: {strays:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Removals race the same way additions do, and must be serialised with them.
#[test]
fn concurrent_adds_and_removes_leave_a_consistent_registry() {
    let dir = scratch("mixed");
    // Seed the registry so the removers have something to remove.
    Workspace::update(&dir, |workspace| {
        for writer in 0..WRITERS {
            workspace.add(
                format!("seed{writer}"),
                PathBuf::from(format!("/tmp/seed{writer}")),
            );
        }
    })
    .expect("seeding must succeed");

    let root = Arc::new(dir.clone());
    let handles: Vec<_> = (0..WRITERS)
        .map(|writer| {
            let root = Arc::clone(&root);
            thread::spawn(move || {
                if writer % 2 == 0 {
                    Workspace::update(root.as_path(), |workspace| {
                        workspace.remove(&format!("seed{writer}"));
                    })
                    .map(|_| ())
                } else {
                    Workspace::update(root.as_path(), |workspace| {
                        workspace.add(
                            format!("late{writer}"),
                            PathBuf::from(format!("/tmp/late{writer}")),
                        );
                    })
                    .map(|_| ())
                }
            })
        })
        .collect();

    for (writer, handle) in handles.into_iter().enumerate() {
        assert!(
            handle.join().expect("writer panicked").is_ok(),
            "writer {writer} failed"
        );
    }

    let registry = Workspace::load(&dir).expect("registry must be readable");
    let names: Vec<&str> = registry
        .repos
        .iter()
        .map(|repo| repo.name.as_str())
        .collect();
    for writer in 0..WRITERS {
        if writer % 2 == 0 {
            assert!(
                !names.contains(&format!("seed{writer}").as_str()),
                "seed{writer} was removed but survived: {names:?}"
            );
        } else {
            assert!(
                names.contains(&format!("seed{writer}").as_str()),
                "seed{writer} was never removed but is gone: {names:?}"
            );
            assert!(
                names.contains(&format!("late{writer}").as_str()),
                "late{writer} was added but is missing: {names:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
