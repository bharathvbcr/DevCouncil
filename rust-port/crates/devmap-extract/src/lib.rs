// The parsing frontend. Everything below the `parse` gate needs tree-sitter
// and its grammars; everything above it — the model types, language detection,
// go.mod parsing, the ignore rules — does not, and is what a *query* consumer
// actually uses.
#[cfg(feature = "parse")]
pub mod cache;
pub mod clonesig;
#[cfg(feature = "parse")]
pub mod embedded;
pub mod fallback;
pub mod frameworks;
pub mod gomod;
#[cfg(feature = "parse")]
pub mod langcalls;
#[cfg(feature = "parse")]
pub(crate) mod langdecl;
pub mod languages;
pub mod model;
// Needs the grammars: a notebook's cells are reconstructed and then handed to
// the real extractor, so this module is only meaningful with `parse` on.
#[cfg(feature = "parse")]
pub mod notebook;
#[cfg(feature = "parse")]
pub mod treesitter;
pub mod wiring;

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "parse")]
use rayon::prelude::*;

pub use gomod::{collect_go_modules, git_worktree_root, parse_go_mod, GoModule};
pub use languages::{detect_language, is_ignored_path, is_indexable_source};
pub use model::*;
#[cfg(feature = "parse")]
pub use treesitter::extract_treesitter;

pub struct FileRef<'a> {
    pub path: &'a str,
    pub source: &'a str,
}

pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

/// Marker file of the Cache Directory Tagging Standard.
pub const CACHEDIR_TAG_FILE: &str = "CACHEDIR.TAG";

/// The standard's mandatory first 43 bytes.
///
/// A directory is a cache directory if and only if it holds a `CACHEDIR.TAG`
/// whose content *begins* with exactly this. The signature is checked rather
/// than the filename alone so a source file that happens to be called
/// `CACHEDIR.TAG` cannot silently delete a subtree from the index.
pub const CACHEDIR_TAG_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Whether `dir` is tagged as a cache directory.
///
/// K7: the ignore rules matched a fixed list of directory *names* — `target`,
/// `node_modules`, `dist`, `build` — so a cargo output directory named anything
/// else was walked as source. Measured in generation 779 of this repository's
/// store: 1,041 of 2,363 indexed files were `.fingerprint/*.json` and
/// `.rustc_info.json` under `rust-port/target-serve` and `target-store`, and
/// the daemon had queued 47,000 pending rows from them. Neither was gitignored;
/// neither was named `target`.
///
/// Both carried a `CACHEDIR.TAG`. That is the point of the standard: the tool
/// that created the cache says so, so nothing downstream has to guess a name.
/// cargo, pip, uv, ccache, tox, ruff and pytest all write one.
///
/// A directory this returns true for is skipped whole — not walked, not
/// indexed, not queued. That is safe because the tag is an explicit,
/// machine-written declaration by the tool that owns the directory, and it is
/// checked by signature rather than by filename.
///
/// This is **not** a replacement for [`is_ignored_path`]: many caches carry no
/// tag (this workspace's own long-lived `target/` has none), so the two are
/// complementary. Absence of a tag says nothing.
pub fn is_cache_directory(dir: &Path) -> bool {
    use std::io::Read;

    let Ok(mut file) = fs::File::open(dir.join(CACHEDIR_TAG_FILE)) else {
        return false;
    };
    let mut head = vec![0u8; CACHEDIR_TAG_SIGNATURE.len()];
    // `read_exact`: a file shorter than the signature cannot carry it, and the
    // error path is the same "not a cache directory" answer.
    if file.read_exact(&mut head).is_err() {
        return false;
    }
    head == CACHEDIR_TAG_SIGNATURE
}

/// Memoised ancestor lookup for [`is_cache_directory`].
///
/// One `open` per directory rather than per path. Reconciling the pending queue
/// asks this of every row — 51,136 of them on the live store — and a repository
/// is a few thousand directories deep in total, so the memo turns
/// O(rows x depth) syscalls into O(distinct directories).
#[derive(Debug, Default)]
pub struct CacheDirectoryCache {
    verdict: std::collections::HashMap<String, bool>,
}

impl CacheDirectoryCache {
    /// The repo-relative tagged cache directory containing `relative`, if any.
    ///
    /// `relative` itself is checked too, so passing a directory answers for the
    /// directory. The repository root is deliberately **not** checked: a user
    /// who points `devmap build` at a tagged directory has asked for it, and
    /// refusing the whole tree would be a worse answer than indexing it.
    pub fn tagged_ancestor(&mut self, root: &Path, relative: &str) -> Option<String> {
        let mut prefix = String::new();
        for part in relative.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let tagged = match self.verdict.get(&prefix) {
                Some(known) => *known,
                None => {
                    let known = is_cache_directory(&root.join(&prefix));
                    self.verdict.insert(prefix.clone(), known);
                    known
                }
            };
            if tagged {
                return Some(prefix);
            }
        }
        None
    }
}

/// One-shot [`CacheDirectoryCache::tagged_ancestor`] for a single question.
pub fn cache_directory_for(root: &Path, relative: &str) -> Option<String> {
    CacheDirectoryCache::default().tagged_ancestor(root, relative)
}

/// The contents of a lock, whatever a panic elsewhere did to it.
///
/// E-8: the prune ledger below was read and written through
/// `if let Ok(guard) = lock.lock()`, which silently does *nothing* once the
/// mutex is poisoned — so a panic anywhere under the walk would erase every
/// pruned `CACHEDIR.TAG` subtree from `skipped_paths` and the report would then
/// describe a tree it had not walked, with nothing saying so. A ledger that
/// could not be read must not read as an empty ledger.
///
/// Recovering is right here rather than propagating: poisoning says a *writer*
/// panicked, not that the data is torn. `BTreeSet::insert` has no intermediate
/// state a panic can leave behind, so the set holds every directory recorded
/// before the panic, and reporting those is strictly better than reporting
/// none. The panic itself is not swallowed — it unwinds its own thread as
/// usual.
fn recover_lock<T>(lock: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod prune_ledger_tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    /// E-8: a poisoned ledger must not read as an empty one.
    ///
    /// The pruned-directory set is shared with `filter_entry`, which the
    /// `ignore` crate requires to be `Fn + Send + Sync`, so a mutex is the
    /// honest way to get an answer back out of it. Both ends of that mutex were
    /// spelled `if let Ok(guard) = lock.lock()`, which does *nothing at all*
    /// once a panic has poisoned it: every pruned `CACHEDIR.TAG` subtree would
    /// vanish from `skipped_paths`, and the report would describe a tree it had
    /// not walked with nothing saying so. On this repository that is 1,041 of
    /// 2,363 candidate paths.
    ///
    /// No end-to-end reproduction is possible today and that is deliberate
    /// rather than an omission: `collect_sources_with_report` builds the serial
    /// `Walk`, so a panic inside the closure unwinds out of the function that
    /// owns the mutex and the report is never read. The trap is one edit away —
    /// `build_parallel` is the obvious next move for discovery, and it runs the
    /// same closure on worker threads where one panic poisons the ledger the
    /// survivors keep writing to. This pins the policy at the only level where
    /// it is observable: the guard, and both branches of it.
    #[test]
    fn a_poisoned_prune_ledger_is_recovered_rather_than_silently_dropped() {
        let ledger: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
        super::recover_lock(&ledger).insert("rust-port/target-serve".to_string());

        let writer = Arc::clone(&ledger);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = writer.lock().unwrap();
            panic!("a walk callback panicked while holding the ledger");
        }));
        assert!(panicked.is_err(), "the fixture must actually panic");
        assert!(
            ledger.lock().is_err(),
            "precondition: the ledger is poisoned, which is the case the old \
             `if let Ok(..)` silently skipped"
        );

        let recovered = super::recover_lock(&ledger);
        assert_eq!(
            recovered.len(),
            1,
            "a directory recorded before the panic is still a directory that was \
             pruned, and the report has to say so"
        );
        assert!(recovered.contains("rust-port/target-serve"));
    }
}

/// Canonical source-content identity shared by extraction, cache, and
/// connect-time freshness checks.
pub fn content_hash(source: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    source
        .as_bytes()
        .iter()
        .fold(FNV_OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
        })
}

/// Evaluate Git ignore rules the same way `WalkBuilder` does for a cold build.
///
/// `WalkBuilder.git_ignore(true)` reads `.gitignore` from the git worktree
/// root down, including parents of `root` when the build is rooted in a
/// subdirectory. The watcher must use this same stack or incremental
/// generations admit files the next cold build drops.
///
/// "The same way" includes **tolerating the same broken files**. A rule file
/// is not all-or-nothing: `GitignoreBuilder::add` compiles every line it can
/// and reports the rest as a *partial* error (`ignore-0.4.33`,
/// `src/gitignore.rs:405-434` — the loop never breaks for a bad glob), which is
/// how `WalkBuilder` walks a tree whose `.gitignore` contains a typo like
/// `[z-a]` without raising anything. Treating that return as fatal made every
/// verdict under such a tree an `Err`, the watcher read the `Err` as
/// "ignored", and the incremental index froze while `status` went on reporting
/// `is_fresh: true` — the divergence this comment claims to prevent, in the
/// opposite direction and silent. The unusable lines are reported through
/// [`is_gitignored_reporting`] instead of being thrown away.
pub fn is_gitignored(root: &Path, path: &Path, is_dir: bool) -> anyhow::Result<bool> {
    Ok(is_gitignored_reporting(root, path, is_dir)?.0)
}

/// [`is_gitignored`], plus one diagnostic per rule line that could not be
/// compiled.
///
/// The verdict is computed from the lines that *did* compile, exactly as the
/// cold walker does. The diagnostics exist so a watcher can say which line of
/// which file it is not applying: a rule the developer wrote and the kernel
/// silently drops is precisely the kind of divergence that is invisible from
/// either side. Empty for a well-formed tree, so a caller pays nothing to
/// carry it.
pub fn is_gitignored_reporting(
    root: &Path,
    path: &Path,
    is_dir: bool,
) -> anyhow::Result<(bool, Vec<String>)> {
    let path_abs = path_under_root(root, path)?;
    let (matchers, problems) = ignore_matchers_for(root, &path_abs, is_dir)?;
    Ok((
        matches_ignore(&matchers, &path_abs, is_dir).unwrap_or(false),
        problems,
    ))
}

/// Ignore-rule files that affect `path`, from the git worktree root (or `root`
/// when there is no git metadata) down to the path. Watcher cache stamps use
/// this list so a parent `.gitignore` edit invalidates verdicts.
pub fn ignore_rule_files(root: &Path, path: &Path, is_dir: bool) -> anyhow::Result<Vec<PathBuf>> {
    let path_abs = path_under_root(root, path)?;
    Ok(ignore_rule_bases(root, &path_abs, is_dir)?
        .into_iter()
        .map(|(_, rules)| rules)
        .collect())
}

fn path_under_root(root: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path_abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path_abs = path_abs.canonicalize().unwrap_or(path_abs);
    path_abs
        .strip_prefix(&root_abs)
        .map_err(|_| anyhow::anyhow!("watch path {path_abs:?} is outside root {root_abs:?}"))?;
    Ok(path_abs)
}

fn ignore_rule_bases(
    root: &Path,
    path: &Path,
    is_dir: bool,
) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
    let git_root = git_worktree_root(root)
        .or_else(|| root.canonicalize().ok())
        .unwrap_or_else(|| root.to_path_buf());
    let path_abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path_abs = path_abs.canonicalize().unwrap_or(path_abs);
    let git_root = git_root.canonicalize().unwrap_or(git_root);
    let rel = path_abs
        .strip_prefix(&git_root)
        .unwrap_or(path_abs.as_path());
    let parent = if is_dir {
        rel
    } else {
        rel.parent().unwrap_or_else(|| Path::new(""))
    };

    let mut rules = vec![
        (git_root.clone(), git_root.join(".git/info/exclude")),
        (git_root.clone(), git_root.join(".gitignore")),
    ];
    let mut current = git_root;
    for component in parent.components() {
        current.push(component.as_os_str());
        rules.push((current.clone(), current.join(".gitignore")));
    }
    Ok(rules)
}

fn ignore_matchers_for(
    root: &Path,
    path: &Path,
    is_dir: bool,
) -> anyhow::Result<(Vec<ignore::gitignore::Gitignore>, Vec<String>)> {
    let bases = ignore_rule_bases(root, path, is_dir)?;
    let mut matchers = Vec::new();
    let mut problems = Vec::new();
    for (base, rules) in &bases {
        add_ignore_rules(&mut matchers, &mut problems, base, rules)?;
    }
    Ok((matchers, problems))
}

/// Compile one rule file into a matcher, keeping every line that is valid.
///
/// The `Option<Error>` from `GitignoreBuilder::add` is a *partial* result, not
/// a verdict on the file: it holds one entry per line that failed to compile
/// while the builder retains all the others. It is also how "the file could not
/// be opened at all" is reported — in which case the builder is simply empty
/// and no rule applies, which is again what the cold walker does. Neither case
/// may abort the evaluation: an ignore verdict that fails is a verdict the
/// watcher reads as "ignored", so one typo would freeze the whole index.
fn add_ignore_rules(
    matchers: &mut Vec<ignore::gitignore::Gitignore>,
    problems: &mut Vec<String>,
    base: &Path,
    rules: &Path,
) -> anyhow::Result<()> {
    if !rules.is_file() {
        return Ok(());
    }
    let mut builder = ignore::gitignore::GitignoreBuilder::new(base);
    if let Some(error) = builder.add(rules) {
        problems.push(format!(
            "ignore rules {rules:?} are partly unusable and those lines are not \
             being applied: {error}"
        ));
    }
    matchers.push(builder.build()?);
    Ok(())
}

fn matches_ignore(
    matchers: &[ignore::gitignore::Gitignore],
    path: &Path,
    is_dir: bool,
) -> Option<bool> {
    let mut ignored = None;
    for matcher in matchers {
        let relative = path.strip_prefix(matcher.path()).unwrap_or(path);
        let matched = matcher.matched_path_or_any_parents(relative, is_dir);
        if matched.is_ignore() {
            ignored = Some(true);
        } else if matched.is_whitelist() {
            ignored = Some(false);
        }
    }
    ignored
}

#[cfg(feature = "parse")]
pub fn extract_file(path: &str, source: &str) -> Extraction {
    // Notebooks divert here rather than inside `extract_treesitter`, because
    // what they need is not a different grammar but a different *source*: the
    // code has to be reconstructed out of the JSON before any grammar sees it,
    // and the resulting spans relocated back into the raw file. Handing the
    // reconstructed buffer straight to the extractor would produce symbols
    // whose spans index a string that exists only in memory.
    if notebook::is_notebook(path) {
        return notebook::extract_notebook(path, source, extract_treesitter);
    }
    let lang = detect_language(Path::new(path));
    extract_treesitter(path, lang, source)
}

#[cfg(feature = "parse")]
pub fn extract_all(files: &[FileRef]) -> Vec<Extraction> {
    files
        .par_iter()
        .map(|f| extract_file(f.path, f.source))
        .collect()
}

#[cfg(test)]
mod content_hash_tests {
    use super::content_hash;

    #[test]
    fn content_hash_is_pinned_fnv1a64() {
        assert_eq!(content_hash(""), 0xcbf29ce484222325);
        assert_eq!(content_hash("hello"), 0xa430d84680aabd0b);
    }
}

/// Collect indexable source files under `root` as owned `(relative_path, source)` pairs.
/// Skips non-source paths and unreadable / non-UTF8 files (fail-closed: omit, do not invent).
pub fn collect_sources(root: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let (sources, _) = collect_sources_with_report(root)?;
    Ok(sources)
}

/// Collect source files and report each admitted or rejected candidate.
/// Gitignored paths are rejected by the walker before they become candidates.
pub fn collect_sources_with_report(
    root: &Path,
) -> anyhow::Result<(Vec<(String, String)>, DiscoveryReport)> {
    let mut out = Vec::new();
    let mut report = DiscoveryReport::default();

    // K7: prune tagged cache directories at the directory, not per file.
    //
    // `filter_entry` returning false for a directory stops the walk descending
    // into it, so a cargo output tree costs one `open` instead of a stat and an
    // extension test for each of its tens of thousands of files. Doing it here
    // rather than in `is_indexable_source` is deliberate: that predicate is a
    // pure function of a path string, and this question can only be answered by
    // reading the filesystem.
    //
    // The pruned directories are recorded so the report can say what was
    // skipped wholesale. `Arc<Mutex<_>>` because `filter_entry` takes a
    // `Fn + Send + Sync + 'static`, and this is the honest way to get an answer
    // back out of it.
    let pruned: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new()));
    let walk_root = root.to_path_buf();
    let pruned_writer = std::sync::Arc::clone(&pruned);
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(move |entry| {
            if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                return true;
            }
            // Never prune the root the caller asked for: pointing devmap at a
            // tagged directory is a request, not an accident.
            if entry.path() == walk_root {
                return true;
            }
            if !is_cache_directory(entry.path()) {
                return true;
            }
            if let Ok(relative) = entry.path().strip_prefix(&walk_root) {
                recover_lock(&pruned_writer).insert(relative.to_string_lossy().replace('\\', "/"));
            }
            false
        })
        .build();

    for result in walker {
        let entry = result?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Ok(rel) = p.strip_prefix(root) else {
            continue;
        };
        let Some(rel_str) = rel.to_str() else {
            report.skipped_paths.push((
                rel.to_string_lossy().into_owned(),
                DiscoverySkipReason::NonUtf8Path,
            ));
            continue;
        };
        let rel_str = rel_str.replace('\\', "/");
        if !is_indexable_source(&rel_str) {
            report
                .skipped_paths
                .push((rel_str, DiscoverySkipReason::NonSource));
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                report.skipped_paths.push((
                    rel_str,
                    DiscoverySkipReason::Unreadable {
                        reason: error.to_string(),
                    },
                ));
                continue;
            }
        };
        if metadata.len() > MAX_SOURCE_BYTES {
            report.skipped_paths.push((
                rel_str,
                DiscoverySkipReason::Oversized {
                    bytes: metadata.len(),
                    limit: MAX_SOURCE_BYTES,
                },
            ));
            continue;
        }
        match fs::read_to_string(p) {
            Ok(src) => {
                report.yielded_paths.push(rel_str.clone());
                out.push((rel_str, src));
            }
            Err(error) => report.skipped_paths.push((
                rel_str,
                DiscoverySkipReason::Unreadable {
                    reason: error.to_string(),
                },
            )),
        }
    }
    // Record each pruned cache directory once, as `NonSource`: a build cache is
    // the ordinary case, like a README beside the code, not a gap in coverage.
    // Recording it at all is what keeps the report honest about the subtree it
    // did not walk.
    for directory in recover_lock(&pruned).iter() {
        report
            .skipped_paths
            .push((directory.clone(), DiscoverySkipReason::NonSource));
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    report.yielded_paths.sort();
    report
        .skipped_paths
        .sort_by(|left, right| left.0.cmp(&right.0));
    Ok((out, report))
}

/// Extract every indexable file under `root` (owned paths — no leaks).
#[cfg(feature = "parse")]
pub fn extract_tree(root: &Path) -> anyhow::Result<Vec<Extraction>> {
    let sources = collect_sources(root)?;
    let refs: Vec<FileRef> = sources
        .iter()
        .map(|(path, src)| FileRef {
            path: path.as_str(),
            source: src.as_str(),
        })
        .collect();
    Ok(extract_all(&refs))
}

#[cfg(all(test, feature = "parse"))]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn extract_tree_skips_non_source_and_finds_py() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-extract-{}", stamp));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("pkg")).unwrap();
        fs::write(root.join("pkg/mod.py"), "def foo():\n    return 1\n").unwrap();
        fs::write(root.join("pkg/notes.bin"), b"\x00\x01\x02\xff").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/x.rs"), "fn ignored() {}\n").unwrap();

        let exts = extract_tree(&root).unwrap();
        assert_eq!(exts.len(), 1);
        assert_eq!(exts[0].file_path, "pkg/mod.py");
        assert!(exts[0].symbols.iter().any(|s| s.name == "foo"));

        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod discovery_bound_tests {
    use super::*;

    /// The source size limit is exclusive and is its declared value.
    ///
    /// `metadata.len() > MAX_SOURCE_BYTES` was mutable to `>=`, and the
    /// constant `1024 * 1024` to `1024 + 1024`. The limit is what stops a
    /// generated multi-megabyte file from being parsed and stored; shrunk to
    /// 2 KiB it silently skips most real sources, and every skip is recorded as
    /// `Oversized` rather than failing, so the map just gets quietly smaller.
    #[test]
    fn the_source_size_limit_is_its_declared_value_and_exclusive() {
        assert_eq!(MAX_SOURCE_BYTES, 1_048_576, "1 MiB source ceiling");

        let dir = std::env::temp_dir().join(format!(
            "devmap-size-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Exactly at the limit is admitted.
        let at = dir.join("at.py");
        std::fs::write(&at, "#".repeat(MAX_SOURCE_BYTES as usize)).unwrap();
        // One byte past is skipped as oversized.
        let over = dir.join("over.py");
        std::fs::write(&over, "#".repeat(MAX_SOURCE_BYTES as usize + 1)).unwrap();

        let (sources, report) = collect_sources_with_report(&dir).unwrap();
        assert!(
            sources.iter().any(|(path, _)| path.ends_with("at.py")),
            "a source exactly at the limit must be admitted"
        );
        assert!(
            !sources.iter().any(|(path, _)| path.ends_with("over.py")),
            "a source past the limit must not be admitted"
        );
        assert!(
            report.skipped_paths.iter().any(|(path, reason)| {
                path.ends_with("over.py") && matches!(reason, DiscoverySkipReason::Oversized { .. })
            }),
            "the skip must be recorded as Oversized, not silently dropped: {:?}",
            report.skipped_paths
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
