//! `.ignore` binds the daemon as it binds the build.
//!
//! The audit's reproduction (2026-10-09, `ts2`): `.ignore` listed `skipme.ts`,
//! the daemon enqueued and drained `src/skipme.ts` the moment it was created,
//! `search skipme` found it, `status` read stale, and the next `devmap build`
//! dropped it — `Changes: +0 ~0 -1` — until the next edit re-added it.

mod support_ignored;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_named_in_dot_ignore_is_not_enqueued_and_status_stays_fresh() {
    let root = support_ignored::scratch("dot-ignore");
    std::fs::write(root.join(".ignore"), "x.ts\n").unwrap();
    std::fs::write(root.join("src/keep.ts"), "export const keep = 1;\n").unwrap();
    support_ignored::excluded_path_stays_out(&root, "src/x.ts", "src/y.ts").await;
    let _ = std::fs::remove_dir_all(&root);
}
