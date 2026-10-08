//! `current_git_head` against an honest git, through the public API.
//!
//! Moved out of `db.rs`'s inline `git_head_tests` module. Its companion — the
//! stalled-git deadline test — drives the private bounded runner, so it stays
//! in `src/db/tests/git_head_tests.rs` as a child of `db`.

use devmap_extract::subprocess::GIT_HEAD_DEADLINE;
use devmap_store::current_git_head;

#[test]
fn a_real_git_head_still_validates_normally() {
    // Positive control: the deadline path must not have broken honest git.
    // Any directory works — /tmp is outside a repo only if git errors, so
    // use this crate's own manifest dir which IS in a repository when the
    // workspace is checked out; fall back to asserting the failure shape
    // otherwise. Either way it must return quickly and cleanly.
    let started = std::time::Instant::now();
    let result = current_git_head(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(started.elapsed() < GIT_HEAD_DEADLINE);
    match result {
        Ok(head) => assert!(
            (7..=64).contains(&head.len()) && head.bytes().all(|b| b.is_ascii_hexdigit()),
            "a real HEAD must pass validation: {head:?}"
        ),
        Err(error) => assert!(
            !error.to_string().contains("killed"),
            "an honest fast failure must not be a kill: {error}"
        ),
    }
}
