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
}
