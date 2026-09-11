//! `CacheDirectoryCache` answered questions it was never asked.
//!
//! The struct memoises `is_cache_directory` per repo-relative prefix so that
//! reconciling 51,136 pending rows costs one `open` per directory instead of
//! one per row. Two things that memo and its walk got wrong:
//!
//! * **The memo key carries no root.** `verdict: HashMap<String, bool>` is
//!   keyed on `"pkg"`, not on `(root, "pkg")`, while `root` arrives as a
//!   parameter on every call. One instance asked about two roots answers the
//!   second from the first's cache.
//! * **`..` walks out of the root.** Components are joined onto `root` without
//!   being canonicalised, so `src/../x` probes `root` itself — which the
//!   doc comment states is *deliberately* never checked, because a user who
//!   points `devmap build` at a tagged directory has asked for it.
//!
//! Both make a build cache verdict wrong in the direction that costs coverage:
//! a real source file refused as a build artifact, skipped whole, never
//! indexed, and absent from every downstream answer.

use devmap_extract::{CacheDirectoryCache, CacheVerdict, CACHEDIR_TAG_SIGNATURE};
use std::fs;
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-cachedir-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn tag(dir: &PathBuf) {
    fs::create_dir_all(dir).unwrap();
    let mut body = CACHEDIR_TAG_SIGNATURE.to_vec();
    body.extend_from_slice(b"\n# created by a test\n");
    fs::write(dir.join("CACHEDIR.TAG"), body).unwrap();
}

/// One cache, two roots: the second root gets the first root's answer.
///
/// Root A's `pkg/` is a tagged build cache; root B's `pkg/` is ordinary source.
/// Asking about `pkg/mod.rs` under B after asking under A must not inherit A's
/// verdict — B's file is source, and a `true` here removes it from the index.
#[test]
fn one_cache_asked_about_two_roots_does_not_answer_for_the_wrong_one() {
    let root_a = scratch("two-roots-a");
    let root_b = scratch("two-roots-b");
    tag(&root_a.join("pkg"));
    fs::create_dir_all(root_b.join("pkg")).unwrap();
    fs::write(root_b.join("pkg/mod.rs"), "fn real() {}\n").unwrap();

    let mut caches = CacheDirectoryCache::default();
    assert_eq!(
        caches.tagged_ancestor(&root_a, "pkg/mod.rs"),
        CacheVerdict::Inside("pkg".to_string()),
        "root A's pkg carries CACHEDIR.TAG",
    );
    assert_eq!(
        caches.tagged_ancestor(&root_b, "pkg/mod.rs"),
        CacheVerdict::Outside,
        "root B's pkg is ordinary source; it must not inherit root A's verdict",
    );
    // ...and going back is not sticky either: the memo follows whichever root
    // asked last, so alternating roots keeps giving each its own answer.
    assert_eq!(
        caches.tagged_ancestor(&root_a, "pkg/mod.rs"),
        CacheVerdict::Inside("pkg".to_string()),
    );

    let _ = fs::remove_dir_all(&root_a);
    let _ = fs::remove_dir_all(&root_b);
}

/// `..` must not reach the repository root the walk documents it never checks.
///
/// The doc comment on `tagged_ancestor` states the root is deliberately exempt.
/// A `..` component defeats that: `root.join("src/..")` *is* the root, so a
/// repository whose own top level carries a tag refuses every path containing
/// `..` — and reports the refusal as the repo-relative directory `src/..`,
/// which names nothing.
#[test]
fn a_dot_dot_component_does_not_walk_back_to_the_exempt_root() {
    let root = scratch("dotdot-root");
    tag(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let mut caches = CacheDirectoryCache::default();
    assert_eq!(
        caches.tagged_ancestor(&root, "src/main.rs"),
        CacheVerdict::Outside,
        "the root is exempt, so a plain path under it is not inside a cache",
    );
    assert_eq!(
        caches.tagged_ancestor(&root, "src/../src/main.rs"),
        CacheVerdict::NotRepoRelative("contains a `..` component"),
        "`..` must not make the exempt root answer for the path",
    );

    let _ = fs::remove_dir_all(&root);
}

/// `..` must not probe outside the repository at all.
///
/// `--affected` hands this raw command-line strings. A leading `..` walks the
/// join above `root`, so the verdict for a file in this repository is decided
/// by a directory in a sibling checkout — and the returned string is offered
/// to the caller as a *repo-relative* cache directory.
#[test]
fn a_leading_dot_dot_does_not_probe_a_sibling_checkout() {
    let parent = scratch("escape-parent");
    let root = parent.join("repo");
    fs::create_dir_all(&root).unwrap();
    tag(&parent.join("sibling"));

    let mut caches = CacheDirectoryCache::default();
    assert_eq!(
        caches.tagged_ancestor(&root, "../sibling/artifact.rs"),
        CacheVerdict::NotRepoRelative("contains a `..` component"),
        "a path outside the root is not a repo-relative cache directory",
    );

    let _ = fs::remove_dir_all(&parent);
}

/// A refusal is not an all-clear.
///
/// This is the state the two-valued return could not express, so unlike its
/// three neighbours it was never watched failing: before `CacheVerdict` there
/// was no `NotRepoRelative` for a test to assert. It is pinned because the
/// whole point of the enum is that a caller can tell "checked, clean" from
/// "never checked" — a plain `PartialEq` on the two would have been enough for
/// a caller to conflate them, and the callers refuse on both.
#[test]
fn a_path_the_walk_refused_is_distinguishable_from_a_clean_one() {
    let root = scratch("refusal-vs-clean");
    fs::create_dir_all(root.join("src")).unwrap();

    let mut caches = CacheDirectoryCache::default();
    let clean = caches.tagged_ancestor(&root, "src/main.rs");
    let refused = caches.tagged_ancestor(&root, "/etc/passwd");
    assert_eq!(clean, CacheVerdict::Outside);
    assert_eq!(
        refused,
        CacheVerdict::NotRepoRelative("is an absolute path"),
        "an absolute path must not be silently reinterpreted as `root/etc/passwd`",
    );
    assert_ne!(clean, refused, "the two answers must not compare equal");

    // The root itself stays exempt through both spellings that name it.
    tag(&root);
    let mut fresh = CacheDirectoryCache::default();
    assert_eq!(fresh.tagged_ancestor(&root, "."), CacheVerdict::Outside);
    assert_eq!(fresh.tagged_ancestor(&root, ""), CacheVerdict::Outside);

    let _ = fs::remove_dir_all(&root);
}
