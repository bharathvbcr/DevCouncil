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

pub fn select(
    root: &Path,
    max_files: usize,
    force_walk: bool,
) -> Result<Selection, String> {
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
    Ok(Selection {
        files: walk(root, max_files)?,
        truncated: false,
        via_walk: true,
    })
}

fn list(root: &Path, max_files: usize) -> Result<Selection, String> {
    let response = dc_grep::list_files(&dc_grep::ListRequest {
        root: root.to_path_buf(),
        path: String::new(),
        max_results: max_files,
        include_ignored: false,
        max_file_bytes: MAX_FILE_BYTES,
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
    walk_into(root, max_files, &mut found)?;
    Ok(found)
}

fn walk_into(dir: &Path, max_files: usize, found: &mut Vec<PathBuf>) -> Result<(), String> {
    if found.len() >= max_files {
        return Ok(());
    }
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
        found.push(path);
        if found.len() >= max_files {
            eprintln!("note: stopping at {max_files} files; pass --max-files to raise it.");
            return Ok(());
        }
    }
    for sub in subdirs {
        walk_into(&sub, max_files, found)?;
        if found.len() >= max_files {
            break;
        }
    }
    Ok(())
}

fn is_source(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let dotted = format!(".{}", ext.to_ascii_lowercase());
    SOURCE_SUFFIXES.contains(&dotted.as_str())
}
