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
    /// Why this file: Python, or the TypeScript fallback.
    pub choice: Choice,
}

/// How the churn target was found. The TypeScript fallback changes what the
/// soak measures (a different extractor, a different appended function), so a
/// run's output says which one it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The first `.py` file in name order.
    Python,
    /// No `.py` file exists, so the first `.ts` file.
    TypeScriptFallback,
}

/// Directories that hold dependencies, build output or version control
/// rather than the corpus's own source, at any depth. A target inside one
/// would be a file the soak rewrites and the build does not index (or one a
/// dependency install would overwrite), so the cycle's symbol would never be
/// found.
const SKIPPED_DIRS: [&str; 4] = ["node_modules", "target", "dist", ".git"];

/// The first `.py` file under `root`, in name order, outside any dotted
/// top-level directory; failing that the first `.ts` file (not a `.d.ts`),
/// for a corpus with no Python, such as GitPulse. Any dotted directory, not
/// `.devcouncil` by name: the state directory is whichever one the kernel
/// picks, and a churn target inside it would be rewritten by the build it is
/// supposed to perturb. Python wins whenever it exists, so the corpora the
/// soak has always run on keep their target. Directories named in
/// [`SKIPPED_DIRS`] are never entered.
pub fn find_target(root: &Path) -> Option<PathBuf> {
    choose_target(root).map(|(path, _)| path)
}

/// [`find_target`], and which of the two rules picked the file.
pub fn choose_target(root: &Path) -> Option<(PathBuf, Choice)> {
    walk(root, true, "py")
        .map(|path| (path, Choice::Python))
        .or_else(|| walk(root, true, "ts").map(|path| (path, Choice::TypeScriptFallback)))
}

fn wanted(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|e| e == extension)
        && !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(".d.ts"))
}

fn walk(dir: &Path, top: bool, extension: &str) -> Option<PathBuf> {
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
            if SKIPPED_DIRS.iter().any(|skipped| name == *skipped) {
                continue;
            }
            if let Some(found) = walk(&path, false, extension) {
                return Some(found);
            }
        } else if kind.is_file() && wanted(&path, extension) {
            return Some(path);
        }
    }
    None
}

impl Corpus {
    /// Pick the target under `root` and keep a pristine copy beside it.
    pub fn open(root: &Path, tag: u32) -> Result<Self, String> {
        let (target, choice) = choose_target(root).ok_or("no target file")?;
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
            choice,
        })
    }

    /// The one output line that names the churn file and the rule that chose
    /// it.
    pub fn describe(&self) -> String {
        let why = match self.choice {
            Choice::Python => "first .py file in name order",
            Choice::TypeScriptFallback => "TypeScript fallback: no .py file under the corpus root",
        };
        format!("soak churn file: {} ({why})", self.relative)
    }

    /// The text appended for cycle `i`: a function the kernel must index, in
    /// the target's own language.
    pub fn cycle_text(&self, i: u64) -> String {
        if self.target.extension().is_some_and(|e| e == "ts") {
            format!("\n// soak cycle {i}\nfunction _soak_{i}(): number {{\n  return {i};\n}}\n")
        } else {
            format!("\n# soak cycle {i}\ndef _soak_{i}():\n    return {i}\n")
        }
    }

    /// Append cycle `i`'s function: a new symbol the kernel must index.
    pub fn append_cycle(&self, i: u64) -> io::Result<()> {
        let mut file = fs::OpenOptions::new().append(true).open(&self.target)?;
        write!(file, "{}", self.cycle_text(i))
    }

    /// The daemon `impact` frame for cycle `i`'s own symbol, as the
    /// `file::symbol` id the kernel treats as unambiguous. A bare name such as
    /// `main` matches every same-named symbol in the corpus (29 in the default
    /// one) and the kernel refuses an ambiguous target, so the soak asks about
    /// the one symbol it knows is unique: the function it just appended.
    pub fn impact_frame(&self, i: u64) -> String {
        serde_json::json!({
            "version": 1,
            "cmd": "impact",
            "target": format!("{}::_soak_{i}", self.relative),
        })
        .to_string()
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
    fn a_corpus_with_no_python_falls_back_to_the_first_typescript_file() {
        let root = tree(
            "ts",
            &[
                ".x/a.ts",
                "a/types.d.ts",
                "a/b.rs",
                "src/main.ts",
                "src/z.ts",
            ],
        );
        assert_eq!(find_target(&root), Some(root.join("src/main.ts")));
        let corpus = Corpus::open(&root, 11).expect("open");
        assert_eq!(corpus.relative, "src/main.ts");
        corpus.append_cycle(2).expect("append");
        assert_eq!(
            fs::read_to_string(&corpus.target).expect("read"),
            "x = 1\n\n// soak cycle 2\nfunction _soak_2(): number {\n  return 2;\n}\n"
        );
        corpus.restore().expect("restore");
        assert_eq!(fs::read_to_string(&corpus.target).expect("read"), "x = 1\n");
        fs::remove_dir_all(&root).expect("cleanup");
        // Python still wins when both exist.
        let both = tree("both", &["a.ts", "z/b.py"]);
        assert_eq!(find_target(&both), Some(both.join("z/b.py")));
        fs::remove_dir_all(&both).expect("cleanup");
    }

    #[test]
    fn dependency_and_build_directories_are_never_a_churn_target() {
        // Only a vendored file exists: no target, rather than a dependency
        // the soak would rewrite and the build would not index.
        for dir in [
            "node_modules",
            "target",
            "dist",
            "pkg/node_modules/x",
            "a/.git",
        ] {
            let root = tree(
                "vendored",
                &[&format!("{dir}/v.ts"), &format!("{dir}/v.py")],
            );
            assert_eq!(find_target(&root), None, "picked a file under {dir}");
            fs::remove_dir_all(&root).expect("cleanup");
        }
        // A real source file sorts after the vendored one and still wins.
        let root = tree("beside", &["node_modules/a.ts", "dist/a.ts", "src/z.ts"]);
        assert_eq!(find_target(&root), Some(root.join("src/z.ts")));
        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn the_choice_says_whether_it_was_the_fallback() {
        let both = tree("why-both", &["a.ts", "z/b.py"]);
        let corpus = Corpus::open(&both, 21).expect("open");
        assert_eq!(
            corpus.describe(),
            "soak churn file: z/b.py (first .py file in name order)"
        );
        fs::remove_dir_all(&both).expect("cleanup");
        let ts = tree("why-ts", &["src/main.ts"]);
        let corpus = Corpus::open(&ts, 22).expect("open");
        assert_eq!(
            corpus.describe(),
            "soak churn file: src/main.ts (TypeScript fallback: no .py file under the corpus root)"
        );
        fs::remove_dir_all(&ts).expect("cleanup");
    }

    #[test]
    fn the_impact_frame_names_the_cycle_symbol_by_file_and_name() {
        let root = tree("impact", &["pkg/main.py"]);
        let corpus = Corpus::open(&root, 9).expect("open");
        let frame: serde_json::Value =
            serde_json::from_str(&corpus.impact_frame(4)).expect("valid json");
        assert_eq!(frame["cmd"], "impact");
        assert_eq!(frame["version"], 1);
        assert_eq!(frame["target"], "pkg/main.py::_soak_4");
        fs::remove_dir_all(&root).expect("cleanup");
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
