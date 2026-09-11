//! Failure semantics for the native capacity harness's simulated editors.

use std::io;
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

#[path = "../examples/worktree_stress/support.rs"]
mod support;

use support::{
    classify_startup, finalize_pass_flag, graph_proves_equivalence, require_ipc_probe,
    retry_file_operation, AdmissionTimeout, ProbeAdmission, StartBarrier, StartupDecision,
};

fn windows_error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

#[test]
fn transient_sharing_retries_without_dropping_the_edit() {
    let pending = Mutex::new(vec![
        Err(windows_error(32)),
        Err(windows_error(5)),
        Ok("saved"),
    ]);
    let retries = AtomicUsize::new(0);
    let result = retry_file_operation(
        || {
            let mut pending = pending.lock().expect("pending");
            pending.remove(0)
        },
        true,
        5.0,
        || {
            retries.fetch_add(1, Ordering::Relaxed);
        },
    )
    .expect("edit lands");
    assert_eq!(result, "saved");
    assert_eq!(retries.load(Ordering::Relaxed), 2);
    assert!(pending.lock().expect("pending").is_empty());
}

#[test]
fn permanent_denial_is_bounded_and_propagated() {
    let error = windows_error(5);
    let calls = AtomicUsize::new(0);
    let start = Instant::now();
    let caught = retry_file_operation(
        || {
            calls.fetch_add(1, Ordering::Relaxed);
            Err::<(), _>(io::Error::from_raw_os_error(5))
        },
        true,
        0.02,
        || {},
    )
    .expect_err("permanent denial must surface");
    assert_eq!(caught.raw_os_error(), error.raw_os_error());
    assert!(calls.load(Ordering::Relaxed) > 1);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn zero_budget_and_other_errors_are_never_retried() {
    let cases: &[(bool, io::Error, f64)] = &[
        (false, windows_error(5), 5.0),
        (true, windows_error(2), 5.0),
        (true, io::Error::other("I/O failure, not WinError 5"), 5.0),
        (true, windows_error(33), 0.0),
    ];
    for (windows, error, budget) in cases {
        let calls = AtomicUsize::new(0);
        let raw = error.raw_os_error();
        let kind = error.kind();
        let caught = retry_file_operation(
            || {
                calls.fetch_add(1, Ordering::Relaxed);
                if let Some(code) = raw {
                    Err::<(), _>(io::Error::from_raw_os_error(code))
                } else {
                    Err(io::Error::new(kind, "I/O failure, not WinError 5"))
                }
            },
            *windows,
            *budget,
            || {},
        )
        .expect_err("must not retry");
        if let Some(code) = raw {
            assert_eq!(caught.raw_os_error(), Some(code));
        } else {
            assert_eq!(caught.kind(), kind);
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn the_budget_cannot_be_disabled_or_made_unbounded() {
    for budget in [-1.0, 6.0, f64::INFINITY, f64::NAN] {
        let err = retry_file_operation(|| Ok(()), true, budget, || {}).expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}

#[test]
fn parallel_editors_cannot_create_unbounded_probe_processes() {
    let admission = Arc::new(ProbeAdmission::new(4, 30.0).expect("admission"));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let lock = Arc::new(Mutex::new(()));
    thread::scope(|scope| {
        let mut joins = Vec::new();
        for _ in 0..256 {
            let admission = Arc::clone(&admission);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            let lock = Arc::clone(&lock);
            joins.push(scope.spawn(move || {
                admission
                    .run(|| {
                        {
                            let _g = lock.lock().expect("peak");
                            let now = active.fetch_add(1, Ordering::Relaxed) + 1;
                            peak.fetch_max(now, Ordering::Relaxed);
                        }
                        thread::sleep(Duration::from_millis(2));
                        active.fetch_sub(1, Ordering::Relaxed);
                        Ok::<_, io::Error>(1)
                    })
                    .expect("probe")
            }));
        }
        let sum: usize = joins.into_iter().map(|j| j.join().expect("join")).sum();
        assert_eq!(sum, 256);
        assert!(peak.load(Ordering::Relaxed) <= 4);
        assert_eq!(admission.peak(), peak.load(Ordering::Relaxed));
        assert_eq!(active.load(Ordering::Relaxed), 0);
    });
}

#[test]
fn timeout_and_failure_do_not_leak_or_create_slots() {
    let admission = ProbeAdmission::new(1, 0.0).expect("admission");
    let inner = admission.run(|| {
        match admission.run(|| -> Result<(), io::Error> {
            panic!("over-admitted");
        }) {
            Err(err) if err.kind() == io::ErrorKind::TimedOut => {}
            other => panic!("expected admission timeout, got {other:?}"),
        }
        Err::<i32, io::Error>(io::Error::other("probe creation refused"))
    });
    let err = inner.expect_err("outer failure");
    assert_eq!(err.kind(), io::ErrorKind::Other);
    assert_eq!(admission.run(|| Ok::<_, io::Error>(7)).expect("slot"), 7);
    assert_eq!(admission.peak(), 1);
}

#[test]
fn unbounded_or_invalid_admission_is_refused() {
    for workers in [0, 33] {
        assert!(ProbeAdmission::new(workers, 1.0).is_err());
    }
    for timeout in [-1.0, 31.0, f64::INFINITY, f64::NAN] {
        assert!(ProbeAdmission::new(1, timeout).is_err());
    }
    let _ok: Result<(), AdmissionTimeout> = Ok(());
}

#[test]
fn windows_requires_ipc_probe_and_unix_does_not() {
    let probe = Path::new("/tmp/ipc_probe");
    require_ipc_probe(true, None).expect_err("Windows without a probe");
    require_ipc_probe(true, Some(probe)).expect("Windows with a probe");
    require_ipc_probe(false, None).expect("Unix without a probe");
    require_ipc_probe(false, Some(probe)).expect("Unix with a probe");
}

#[test]
fn an_empty_graph_cannot_prove_incremental_equals_cold() {
    assert!(!graph_proves_equivalence(0, 0));
    assert!(!graph_proves_equivalence(3, 0));
    assert!(!graph_proves_equivalence(0, 1));
    assert!(graph_proves_equivalence(1, 1));
}

#[test]
fn a_dead_child_is_not_ready_even_if_the_endpoint_still_exists() {
    assert_eq!(
        classify_startup(true, true, Some(true)),
        StartupDecision::Exited
    );
    assert_eq!(
        classify_startup(false, true, Some(true)),
        StartupDecision::Ready
    );
    assert_eq!(
        classify_startup(false, true, Some(false)),
        StartupDecision::Retry
    );
    assert_eq!(classify_startup(false, false, None), StartupDecision::Retry);
}

#[test]
fn a_failed_run_cannot_keep_a_passing_receipt() {
    assert!(finalize_pass_flag(true, false, false));
    assert!(!finalize_pass_flag(true, true, false));
    assert!(!finalize_pass_flag(true, false, true));
    assert!(!finalize_pass_flag(false, false, false));
}

#[test]
fn a_missing_editor_cannot_deadlock_the_start_barrier() {
    let barrier = StartBarrier::new(2, 0.05).expect("barrier");
    let start = Instant::now();
    let err = barrier.wait().expect_err("must time out");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));
    let err = barrier.wait().expect_err("broken for the late party too");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
}

#[test]
fn timeout_zero_admission_refuses_a_second_slot_immediately() {
    let admission = ProbeAdmission::new(1, 0.0).expect("admission");
    let start = Instant::now();
    admission
        .run(|| {
            let inner = admission.run(|| -> Result<(), io::Error> { Ok(()) });
            assert_eq!(
                inner.expect_err("no spare slot").kind(),
                io::ErrorKind::TimedOut
            );
            Ok::<_, io::Error>(())
        })
        .expect("outer slot");
    assert!(start.elapsed() < Duration::from_millis(200));
}
