//! The file the soak perturbs, and the copy that restores it.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The churn target and its pristine copy.
#[derive(Debug, Clone)]
pub struct Corpus {
    pub target: PathBuf,
    /// `target` relative to the corpus root, `/`-separated: the path the
    /// kernel reports symbols at.
    pub relative: String,
    pub orig: PathBuf,
}

/// The first `.py` file under `root`, in name order, outside any dotted
/// top-level directory. Any dotted one, not `.devcouncil` by name: the state
/// directory is whichever one the kernel picks, and a churn target inside it
/// would be rewritten by the build it is supposed to perturb.
pub fn find_target(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, top: bool) -> Option<PathBuf> {
        let mut entries: Vec<_> = fs::read_dir(dir).ok()?.filter_map(Result::ok).collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name();
            if top && name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if let Some(found) = walk(&path, false) {
                    return Some(found);
                }
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "py") {
                return Some(path);
            }
        }
        None
    }
    walk(root, true)
}

impl Corpus {
    /// Pick the target under `root` and keep a pristine copy beside it.
    pub fn open(root: &Path, tag: u32) -> Result<Self, String> {
        let target = find_target(root).ok_or("no target file")?;
        let relative = target
            .strip_prefix(root)
            .map_err(|_| "the target is outside the corpus".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let orig = root.join(format!(".soak_orig.{tag}"));
        fs::copy(&target, &orig).map_err(|e| format!("cannot copy {}: {e}", target.display()))?;
        Ok(Self {
            target,
            relative,
            orig,
        })
    }

    /// Append cycle `i`'s function: a new symbol the kernel must index.
    pub fn append_cycle(&self, i: u64) -> io::Result<()> {
        let mut file = fs::OpenOptions::new().append(true).open(&self.target)?;
        write!(
            file,
            "\n# soak cycle {i}\ndef _soak_{i}():\n    return {i}\n"
        )
    }

    /// Put the pristine bytes back, touching the file only when they differ:
    /// a rewrite of identical bytes is still an edit to a watcher.
    pub fn restore(&self) -> io::Result<()> {
        let pristine = fs::read(&self.orig)?;
        if fs::read(&self.target).ok().as_deref() != Some(pristine.as_slice()) {
            fs::write(&self.target, pristine)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("soak-corpus-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for f in files {
            let path = root.join(f);
            fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            fs::write(path, "x = 1\n").expect("file");
        }
        root
    }

    #[test]
    fn the_target_skips_dotted_top_level_directories_only() {
        let root = tree(
            "skip",
            &[".devmap/a.py", ".x/b.py", "pkg/.hidden/c.py", "pkg/d.py"],
        );
        assert_eq!(find_target(&root), Some(root.join("pkg/.hidden/c.py")));
        fs::remove_dir_all(&root).expect("cleanup");
        let none = tree("none", &[".devmap/a.py", "readme.md"]);
        assert_eq!(find_target(&none), None);
        fs::remove_dir_all(&none).expect("cleanup");
    }

    #[test]
    fn a_cycle_is_appended_and_restore_returns_the_pristine_bytes() {
        let root = tree("cycle", &["main.py"]);
        let corpus = Corpus::open(&root, 7).expect("open");
        assert_eq!(corpus.relative, "main.py");
        corpus.append_cycle(3).expect("append");
        assert_eq!(
            fs::read_to_string(&corpus.target).expect("read"),
            "x = 1\n\n# soak cycle 3\ndef _soak_3():\n    return 3\n"
        );
        corpus.restore().expect("restore");
        assert_eq!(fs::read_to_string(&corpus.target).expect("read"), "x = 1\n");
        let before = fs::metadata(&corpus.target)
            .and_then(|m| m.modified())
            .expect("mtime");
        std::thread::sleep(std::time::Duration::from_millis(20));
        corpus.restore().expect("restore again");
        let after = fs::metadata(&corpus.target)
            .and_then(|m| m.modified())
            .expect("mtime");
        assert_eq!(before, after, "restoring identical bytes rewrote the file");
        fs::remove_dir_all(&root).expect("cleanup");
    }
}
