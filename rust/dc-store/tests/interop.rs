//! Interoperability with the incumbent, on one file.
//!
//! This is Phase 2's real gate. Every other test in this crate proves the Rust
//! store is self-consistent, which is necessary and nowhere near sufficient:
//! during the migration `dev tasks` and the harness read and write the same
//! `.devcouncil/state.sqlite`, and a schema or timestamp divergence would let
//! both sides believe they hold the same task.
//!
//! So the test drives both. Rust acquires, Python reads it back through
//! DevCouncil's own `TaskLeaseRepository`, and the reverse. If DevCouncil is
//! not importable the test skips loudly rather than passing quietly — a skipped
//! interop check must never look like a passed one.

use std::path::{Path, PathBuf};
use std::process::Command;

use dc_store::{AcquireRequest, LeaseCode, Store};

/// The DevCouncil checkout this crate is part of: `<repo>/rust/dc-store`, so
/// the repository root is two levels up.
///
/// This used to walk three levels up and re-enter by the name `DevCouncil`,
/// which was correct while the crate lived in a *sibling* repository. Now that
/// it lives inside DevCouncil, resolving the root by name would be right only
/// by a coincidence of depth, and only for a checkout literally named
/// `DevCouncil` — a worktree or a clone under any other name would resolve to
/// a directory that does not exist and take the skip path below, reporting
/// "no venv" for what is really "the test cannot find the repository it is in".
fn devcouncil_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate is at <repo>/rust/dc-store, so it has two ancestors")
}

/// Locates a Python interpreter with DevCouncil importable, or None.
fn devcouncil_python() -> Option<(PathBuf, PathBuf)> {
    let repo = devcouncil_root();
    let python = repo.join(".venv/bin/python");
    let src = repo.join("src");
    if python.is_file() && src.is_dir() {
        Some((python, src))
    } else {
        None
    }
}

/// Reports that interop could not be exercised here, and decides whether that
/// is tolerable.
///
/// The file header promises a skipped interop check never looks like a passed
/// one, but `eprintln!` + `return` is exactly that: `cargo test` prints `ok`.
/// That was survivable when the venv was a sibling repository's private detail.
/// It is not now — the interpreter is `.venv/bin/python` inside this very
/// checkout, so "missing" means an unprovisioned tree rather than an unrelated
/// machine, and the check silently not running is the likeliest way this gate
/// rots. `DC_STORE_REQUIRE_INTEROP=1` turns the skip into a failure so CI can
/// demand the evidence; unset, a contributor without `uv sync` still gets a
/// loud message rather than a broken build.
fn skip_or_fail(test: &str) {
    let message = format!(
        "interop unverified: no {} — DevCouncil's Python side was never driven, \
         so schema and timestamp agreement is UNPROVEN for {test}",
        devcouncil_root().join(".venv/bin/python").display(),
    );
    assert!(
        std::env::var_os("DC_STORE_REQUIRE_INTEROP").is_none(),
        "{message} (DC_STORE_REQUIRE_INTEROP is set, so this is a failure)"
    );
    eprintln!("SKIP: {message}");
}

/// Runs a snippet with DevCouncil on the path, returning stdout.
fn run_python(python: &Path, src: &Path, code: &str) -> String {
    let output = Command::new(python)
        .arg("-c")
        .arg(code)
        .env("PYTHONPATH", src)
        .output()
        .expect("spawn python");
    if !output.status.success() {
        panic!(
            "python failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn temp_db(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-store-interop-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.sqlite");
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn python_reads_a_lease_the_rust_store_wrote() {
    let Some((python, src)) = devcouncil_python() else {
        skip_or_fail("python_reads_a_lease_the_rust_store_wrote");
        return;
    };
    let db = temp_db("rust-writes");

    let store = Store::open(&db).unwrap();
    let lease = store
        .acquire(&AcquireRequest {
            task_id: "TASK-INTEROP".into(),
            owner: "rust-builder".into(),
            client_id: Some("harness".into()),
            ttl_seconds: Some(900),
            ..Default::default()
        })
        .unwrap();

    // DevCouncil's own repository, against the file Rust just wrote.
    let code = format!(
        r#"
from sqlmodel import create_engine, Session
from devcouncil.storage.native import TaskLeaseRepository
engine = create_engine("sqlite:///{db}")
with Session(engine) as s:
    repo = TaskLeaseRepository(s)
    active = repo.active_for_task("TASK-INTEROP")
    assert active is not None, "Python saw no active lease"
    print(active.owner)
    print(active.lease_token)
    print(repo.validate("TASK-INTEROP", "{token}"))
    print(repo.validate("TASK-INTEROP", "wrong-token"))
"#,
        db = db.display(),
        token = lease.token
    );
    let out = run_python(&python, &src, &code);
    let lines: Vec<&str> = out.lines().collect();

    assert_eq!(
        lines[0], "rust-builder",
        "owner did not survive the boundary"
    );
    assert_eq!(lines[1], lease.token, "token did not survive the boundary");
    assert_eq!(lines[2], "True", "Python rejected a token Rust issued");
    assert_eq!(lines[3], "False", "Python accepted a token nobody issued");

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn the_rust_store_reads_a_lease_python_wrote_and_refuses_to_double_book_it() {
    let Some((python, src)) = devcouncil_python() else {
        skip_or_fail("the_rust_store_reads_a_lease_python_wrote_and_refuses_to_double_book_it");
        return;
    };
    let db = temp_db("python-writes");

    // Create the schema from the Rust side, then let Python acquire through it.
    Store::open(&db).unwrap();
    let code = format!(
        r#"
from sqlmodel import create_engine, Session
from devcouncil.storage.native import TaskLeaseRepository
engine = create_engine("sqlite:///{db}")
with Session(engine) as s:
    lease = TaskLeaseRepository(s).acquire("TASK-INTEROP", "python-builder", ttl_seconds=900)
    print(lease.lease_token)
"#,
        db = db.display()
    );
    let token = run_python(&python, &src, &code);

    let store = Store::open(&db).unwrap();
    let active = store
        .active_lease("TASK-INTEROP")
        .unwrap()
        .expect("Rust saw no lease Python wrote");
    assert_eq!(active.owner, "python-builder");
    assert_eq!(active.token, token);

    // The token Python issued validates on the Rust side.
    assert_eq!(
        store.diagnose("TASK-INTEROP", &token).unwrap(),
        LeaseCode::Valid
    );

    // And the mutual exclusion holds across the boundary: the harness must not
    // be able to take a task the Python side is already building.
    let conflict = store.acquire(&AcquireRequest {
        task_id: "TASK-INTEROP".into(),
        owner: "rust-builder".into(),
        ttl_seconds: Some(900),
        ..Default::default()
    });
    assert!(
        conflict.is_err(),
        "the harness double-booked a task Python holds"
    );

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

/// The expiry written by Rust must be interpreted the same way by Python.
/// A timestamp both sides can store but read differently is the subtlest way
/// this migration could go wrong: nothing errors, and the two disagree about
/// when a lease died.
#[test]
fn both_sides_agree_on_when_a_lease_expires() {
    let Some((python, src)) = devcouncil_python() else {
        skip_or_fail("both_sides_agree_on_when_a_lease_expires");
        return;
    };
    let db = temp_db("expiry");

    let store = Store::open(&db).unwrap();
    // A lease whose one-second life has passed. (A negative TTL would be the
    // obvious way to get here, but the store now refuses those: minting a
    // lease that is born expired while reporting success was a defect, not a
    // feature.)
    let lease = store
        .acquire(&AcquireRequest {
            task_id: "TASK-EXPIRED".into(),
            owner: "rust-builder".into(),
            ttl_seconds: Some(1),
            ..Default::default()
        })
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let code = format!(
        r#"
from sqlmodel import create_engine, Session
from devcouncil.storage.native import TaskLeaseRepository
engine = create_engine("sqlite:///{db}")
with Session(engine) as s:
    print(TaskLeaseRepository(s).active_for_task("TASK-EXPIRED") is None)
"#,
        db = db.display()
    );
    assert_eq!(
        run_python(&python, &src, &code),
        "True",
        "Python still considered an expired lease live"
    );

    // Rust reaches the same conclusion, and the token reads as recoverable.
    assert!(store.active_lease("TASK-EXPIRED").unwrap().is_none());
    assert_eq!(
        store.diagnose("TASK-EXPIRED", &lease.token).unwrap(),
        LeaseCode::Expired
    );

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}
