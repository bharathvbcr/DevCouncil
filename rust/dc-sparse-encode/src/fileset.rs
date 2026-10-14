//! Which files get encoded. The listing comes from `dcgrep`'s own walk, so
//! the encoding covers what the index will admit. `--walk` is the fallback
//! that does not read `.gitignore`; the script measured that difference at
//! 498 documents over this repository, and the build then reports them as
//! `lexical_unmatched`.

use std::path::{Path, PathBuf};

/// Text the encoder will read. A binary encoded as text gets confident
/// weights for nothing. Same set as the script.
const SOURCE_SUFFIXES: &[&str] = &[
    ".c", ".cc", ".cpp", ".cs", ".css", ".go", ".h", ".hpp", ".html", ".java", ".js", ".json",
    ".jsx", ".kt", ".lua", ".md", ".mjs", ".php", ".proto", ".py", ".rb", ".rs", ".scala", ".sh",
    ".sql", ".swift", ".toml", ".ts", ".tsx", ".txt", ".vue", ".yaml", ".yml", ".zsh",
];

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".devcouncil",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".next",
    ".cargo",
];

/// The searcher's own size ceiling. A listing that used a different one
/// would encode files the index then declines to open.
pub const MAX_FILE_BYTES: u64 = dc_grep::DEFAULT_MAX_FILE_BYTES;

/// Default document budget. It is the listing ceiling, so "encode this
/// repository" can name every file the index will accept back.
pub const DEFAULT_MAX_FILES: usize = dc_grep::MAX_LIST_RESULTS;

#[derive(Clone, Debug)]
pub struct Selection {
    pub files: Vec<PathBuf>,
    /// The listing stopped early. A prefix encoded under the default budget
    /// is an error; a prefix the operator asked for with `--max-files` is a
    /// warning. The two are not the same request.
    pub truncated: bool,
    pub via_walk: bool,
}

pub fn select(root: &Path, max_files: usize, force_walk: bool) -> Result<Selection, String> {
    if max_files == 0 {
        return Err("--max-files must be at least 1".into());
    }
    if !force_walk {
        match list(root, max_files) {
            Ok(selection) => return Ok(selection),
            Err(err) => {
                eprintln!(
                    "note: could not ask dcgrep for the file list ({err}). \
                     falling back to this encoder's own walk, which does not read \
                     .gitignore; expect lexical_unmatched to be non-zero."
                );
            }
        }
    }
    let mut files = Vec::new();
    let truncated = walk_into(root, max_files, &mut files)?;
    Ok(Selection {
        files,
        truncated,
        via_walk: true,
    })
}

fn list(root: &Path, max_files: usize) -> Result<Selection, String> {
    let response = dc_grep::list_files(&dc_grep::ListRequest {
        root: root.to_path_buf(),
        max_results: max_files,
        max_file_bytes: MAX_FILE_BYTES,
        ..Default::default()
    })?;
    if !response.ok {
        return Err("dcgrep files returned ok: false".into());
    }
    let mut files = Vec::new();
    for rel in response.paths {
        let path = root.join(&rel);
        if is_source(&path) {
            files.push(path);
        }
    }
    Ok(Selection {
        files,
        truncated: response.truncated,
        via_walk: false,
    })
}

/// The script's walk: sorted, skipping hidden and dependency directories,
/// symlinks, and files over the size ceiling. It does not read `.gitignore`.
pub fn walk(root: &Path, max_files: usize) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let _omitted = walk_into(root, max_files, &mut found)?;
    Ok(found)
}

/// `Ok(true)` means an eligible source file was left out. After `found` is
/// full, walking continues only until that file or the end of the tree.
fn walk_into(dir: &Path, max_files: usize, found: &mut Vec<PathBuf>) -> Result<bool, String> {
    let mut entries = Vec::new();
    let read = std::fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in read {
        let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
        entries.push(entry.path());
    }
    entries.sort();
    let mut subdirs = Vec::new();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name) {
                continue;
            }
            subdirs.push(path);
            continue;
        }
        if !meta.is_file() || !is_source(&path) || meta.len() > MAX_FILE_BYTES {
            continue;
        }
        if found.len() >= max_files {
            eprintln!("note: stopping at {max_files} files; pass --max-files to raise it.");
            return Ok(true);
        }
        found.push(path);
    }
    for sub in subdirs {
        if walk_into(&sub, max_files, found)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_source(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let dotted = format!(".{}", ext.to_ascii_lowercase());
    SOURCE_SUFFIXES.contains(&dotted.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Removed on drop, including when an assertion panics.
    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("dc-sparse-fileset-{}-{seq}", std::process::id()));
            fs::create_dir_all(&path)
                .unwrap_or_else(|err| panic!("create {}: {err}", path.display()));
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Restores 0o755 on drop, including after a panic, so `TempTree` can
    /// delete a directory that was chmod'd to 0o000. Declare it after the
    /// `TempTree` it guards: locals drop in reverse declaration order.
    struct RestoreMode {
        path: PathBuf,
    }

    impl RestoreMode {
        fn new(path: PathBuf) -> Self {
            Self { path }
        }
    }

    impl Drop for RestoreMode {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755));
        }
    }

    /// False means `mkfifo` is not on PATH and the caller should skip.
    fn make_fifo(path: &Path) -> bool {
        let output = match std::process::Command::new("mkfifo").arg(path).output() {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping fifo case: mkfifo is not on PATH ({err})");
                return false;
            }
            Err(err) => panic!("mkfifo {}: {err}", path.display()),
        };
        if output.status.success() {
            return true;
        }
        panic!(
            "mkfifo {} exited {}: {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_file(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|err| panic!("create {}: {err}", parent.display()));
        }
        fs::write(&path, body).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    }

    #[test]
    fn capped_walk_of_three_sources_is_truncated() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        write_file(root, "c.rs", "fn c() {}\n");
        let selection = select(root, 2, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            selection.truncated,
            "capped walk omitted c.rs but truncated was false"
        );
        assert_eq!(selection.files, vec![root.join("a.rs"), root.join("b.rs")]);
    }

    #[test]
    fn exact_fit_is_not_truncated() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        let selection = select(root, 2, true).unwrap();
        assert!(selection.via_walk);
        assert!(!selection.truncated);
        assert_eq!(selection.files.len(), 2);
    }

    #[test]
    fn fewer_files_than_the_cap_is_not_truncated() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "only.rs", "fn only() {}\n");
        let selection = select(root, 5, true).unwrap();
        assert!(selection.via_walk);
        assert!(!selection.truncated);
        assert_eq!(selection.files, vec![root.join("only.rs")]);
    }

    #[test]
    fn a_source_in_a_later_directory_is_an_omission() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        write_file(root, "sub/d.rs", "fn d() {}\n");
        let selection = select(root, 2, true).unwrap();
        assert!(selection.via_walk);
        assert!(selection.truncated);
        assert_eq!(selection.files, vec![root.join("a.rs"), root.join("b.rs")]);
        assert!(
            selection
                .files
                .iter()
                .all(|path| path.file_name().is_none_or(|name| name != "d.rs")),
            "omitted sub/d.rs must not be in the selection"
        );
    }

    #[test]
    fn non_source_files_are_not_omissions() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        write_file(root, "notes.bin", "bin\n");
        // `.txt` is in SOURCE_SUFFIXES, so this root file is eligible. It is
        // not a stand-in for a non-source neighbor.
        write_file(root, "skip.txt", "text\n");
        let with_txt = select(root, 2, true).unwrap();
        assert!(with_txt.via_walk);
        assert!(
            with_txt.truncated,
            "skip.txt is a source file and is the omitted third file"
        );
        assert_eq!(with_txt.files, vec![root.join("a.rs"), root.join("b.rs")]);

        let plain = TempTree::new();
        let root = plain.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        write_file(root, "notes.bin", "bin\n");
        write_file(root, "skip.dat", "dat\n");
        write_file(root, "node_modules/nested.rs", "skipped directory\n");
        write_file(root, ".git/hidden.rs", "hidden directory\n");
        let fat = root.join("fat.rs");
        fs::write(&fat, vec![b'x'; (MAX_FILE_BYTES as usize) + 1])
            .unwrap_or_else(|err| panic!("write {}: {err}", fat.display()));
        std::os::unix::fs::symlink(root.join("a.rs"), root.join("link.rs")).unwrap();
        let selection = select(root, 2, true).unwrap();
        assert!(selection.via_walk);
        assert!(!selection.truncated, "non-source files are not omissions");
        assert_eq!(selection.files, vec![root.join("a.rs"), root.join("b.rs")]);
    }

    #[test]
    fn non_ascii_source_name_counts_toward_the_cap() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "naïve.rs", "fn n() {}\n");
        write_file(root, "z.rs", "fn z() {}\n");
        let selection = select(root, 1, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            selection.truncated,
            "naïve.rs is a source file and must occupy the only slot, leaving z.rs out"
        );
        assert_eq!(selection.files, vec![root.join("naïve.rs")]);
    }

    #[test]
    fn cap_filled_in_first_subdirectory_omits_later_sibling() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "early/a.rs", "fn a() {}\n");
        write_file(root, "early/b.rs", "fn b() {}\n");
        write_file(root, "late/c.rs", "fn c() {}\n");
        let kept_a = root.join("early/a.rs");
        let kept_b = root.join("early/b.rs");
        let omitted = root.join("late/c.rs");
        assert!(kept_a < kept_b && kept_b < omitted);
        let selection = select(root, 2, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            selection.truncated,
            "late/c.rs is an eligible file past a cap filled in early/"
        );
        assert_eq!(selection.files, vec![kept_a, kept_b]);
        assert!(selection.files.len() <= 2);
        assert!(
            !selection.files.contains(&omitted),
            "later sibling {:?} was returned: {:?}",
            omitted,
            selection.files
        );
    }

    #[test]
    fn empty_directory_is_not_truncated() {
        let tree = TempTree::new();
        let root = tree.path();
        let selection = select(root, 1, true).unwrap();
        assert!(selection.via_walk);
        assert!(!selection.truncated);
        assert!(selection.files.is_empty());
    }

    #[test]
    fn uppercase_extension_counts_toward_the_cap() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "FILE.RS", "fn upper() {}\n");
        write_file(root, "a.rs", "fn lower() {}\n");
        let upper = root.join("FILE.RS");
        let lower = root.join("a.rs");
        // 'F' (0x46) sorts before 'a' (0x61). Path order is bytes, not locale.
        assert!(
            upper < lower,
            "expected FILE.RS before a.rs; got upper<lower {}",
            upper < lower
        );
        let selection = select(root, 1, true).unwrap();
        assert!(selection.via_walk);
        assert!(selection.truncated, "a.rs is an omitted source file");
        assert_eq!(selection.files, vec![upper]);
        assert!(!selection.files.contains(&lower));
    }

    #[test]
    fn dotfile_source_is_eligible_and_sorted() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, ".secret.rs", "fn secret() {}\n");
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        let secret = root.join(".secret.rs");
        let a = root.join("a.rs");
        let b = root.join("b.rs");
        let mut ranked = vec![secret.clone(), a.clone(), b.clone()];
        ranked.sort();
        assert_eq!(
            ranked[0], secret,
            "dotfile did not sort first; ranked={ranked:?}"
        );
        let selection = select(root, 2, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            selection.truncated,
            "one of the three eligible files must be omitted at max 2"
        );
        assert_eq!(selection.files, ranked[..2].to_vec());
        assert!(
            selection.files.contains(&secret),
            "dotfile sorts first and must be kept: {:?}",
            selection.files
        );
        assert!(!selection.files.contains(&b));
        assert!(selection.files.len() <= 2);
    }

    #[test]
    fn symlink_directory_is_not_an_omission() {
        let outside = TempTree::new();
        write_file(outside.path(), "secret.rs", "fn secret() {}\n");
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        std::os::unix::fs::symlink(outside.path(), root.join("linked")).unwrap();
        let selection = select(root, 2, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            !selection.truncated,
            "following the directory symlink would omit secret.rs"
        );
        assert_eq!(selection.files, vec![root.join("a.rs"), root.join("b.rs")]);
        assert!(
            selection
                .files
                .iter()
                .all(|path| path.starts_with(root) && !path.starts_with(outside.path())),
            "selection left the root: {:?}",
            selection.files
        );
    }

    #[test]
    fn size_ceiling_is_inclusive_and_one_byte_over_is_not() {
        let tree = TempTree::new();
        let root = tree.path();
        let exact = root.join("exact.rs");
        let over = root.join("over.rs");
        let exact_len = MAX_FILE_BYTES as usize;
        fs::write(&exact, vec![b'x'; exact_len])
            .unwrap_or_else(|err| panic!("write {}: {err}", exact.display()));
        fs::write(&over, vec![b'y'; exact_len + 1])
            .unwrap_or_else(|err| panic!("write {}: {err}", over.display()));
        assert_eq!(fs::metadata(&exact).unwrap().len(), MAX_FILE_BYTES);
        assert_eq!(fs::metadata(&over).unwrap().len(), MAX_FILE_BYTES + 1);
        assert!(exact < over);
        let selection = select(root, 1, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            !selection.truncated,
            "one byte over the ceiling is not an omission"
        );
        assert_eq!(selection.files, vec![exact]);
        assert!(!selection.files.contains(&over));
    }

    #[test]
    fn space_and_hash_in_names_count() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a file.rs", "fn spaced() {}\n");
        write_file(root, "b#c.rs", "fn hashed() {}\n");
        let spaced = root.join("a file.rs");
        let hashed = root.join("b#c.rs");
        assert!(
            spaced < hashed,
            "expected 'a file.rs' before 'b#c.rs'; spaced<hashed {}",
            spaced < hashed
        );
        let selection = select(root, 1, true).unwrap();
        assert!(selection.via_walk);
        assert!(selection.truncated);
        assert_eq!(selection.files, vec![spaced]);
        assert!(!selection.files.contains(&hashed));
    }

    #[test]
    fn skipped_and_hidden_directories_are_not_omissions() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "src/keep.rs", "fn keep() {}\n");
        write_file(root, "src/node_modules/x.rs", "fn skipped() {}\n");
        write_file(root, "src/.hidden/y.rs", "fn hidden() {}\n");
        let selection = select(root, 1, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            !selection.truncated,
            "node_modules and a hidden directory are not omissions"
        );
        assert_eq!(selection.files, vec![root.join("src/keep.rs")]);
        assert!(selection.files.iter().all(|path| {
            !path.components().any(|component| {
                component.as_os_str() == "node_modules" || component.as_os_str() == ".hidden"
            })
        }));
    }

    #[test]
    fn walk_returns_at_most_max_files() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a.rs", "fn a() {}\n");
        write_file(root, "b.rs", "fn b() {}\n");
        let found = walk(root, 1).expect("walk stays Result and Ok when the cap cuts the tree");
        assert_eq!(found.len(), 1);
        assert_eq!(found, vec![root.join("a.rs")]);
    }

    #[test]
    fn unreadable_subdirectory_is_an_error() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "visible.rs", "fn visible() {}\n");
        let sealed = root.join("sealed");
        write_file(&sealed, "missed.rs", "fn missed() {}\n");
        // After `tree`, so a panic restores 0o755 before the tree is removed.
        let _restore = RestoreMode::new(sealed.clone());
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000))
            .unwrap_or_else(|err| panic!("chmod {}: {err}", sealed.display()));
        let probe = fs::read_dir(&sealed);
        assert!(
            probe.is_err(),
            "setup failed: {} is still readable, so this would not test read_dir",
            sealed.display()
        );
        match select(root, 5, true) {
            Ok(selection) => panic!(
                "unreadable {} returned Ok truncated={} files={:?}",
                sealed.display(),
                selection.truncated,
                selection.files
            ),
            Err(err) => assert!(
                err.contains(&sealed.display().to_string()),
                "error should name {}: {err}",
                sealed.display()
            ),
        }
    }

    #[test]
    fn fifo_is_not_an_omitted_source() {
        let tree = TempTree::new();
        let root = tree.path();
        write_file(root, "a.rs", "fn a() {}\n");
        let fifo = root.join("pipe.rs");
        if !make_fifo(&fifo) {
            return;
        }
        use std::os::unix::fs::FileTypeExt;
        let meta = fs::symlink_metadata(&fifo)
            .unwrap_or_else(|err| panic!("stat {}: {err}", fifo.display()));
        assert!(meta.file_type().is_fifo(), "mkfifo did not create a fifo");
        assert!(!meta.is_file());
        let selection = select(root, 1, true).unwrap();
        assert!(selection.via_walk);
        assert!(
            !selection.truncated,
            "pipe.rs is a fifo and must not count as an omitted source"
        );
        assert_eq!(selection.files, vec![root.join("a.rs")]);
        assert!(!selection.files.contains(&fifo));
    }
}
