//! Symbol ids in exactly the form the scorer joins on: `<file>::<qualified
//! name>`, and the file path itself for a site outside every function.
//!
//! The names come from DevMap's own extractor (`devmap_extract::extract_file`),
//! the one owner of how a Rust symbol is spelled (`Type.method` for an impl or a
//! trait, `outer.inner` for a nested function, no module segment for an inline
//! module). Re-deriving them here would be a second naming scheme that drifts.
//! What the extractor does NOT decide is which symbol a call binds to: that is
//! rust-analyzer's, and [`Namer`] only names the answer. So a naming defect in
//! the extractor appears on both sides of a label and is not caught by it; a
//! binding defect is.

use std::collections::BTreeMap;

use devmap_extract::model::SymbolKind;

/// A function or method the oracle bound a site to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    /// A method (of an impl or a trait) rather than a free function.
    pub is_method: bool,
}

#[derive(Debug)]
struct Callable {
    start: usize,
    end: usize,
    id: String,
    is_method: bool,
}

/// Every callable of every file read, by file.
#[derive(Debug, Default)]
pub struct Namer {
    by_file: BTreeMap<String, Vec<Callable>>,
}

impl Namer {
    /// Extract every `(path, source)` pair and index its functions and methods.
    pub fn new(files: &[(String, String)]) -> Self {
        let mut by_file = BTreeMap::new();
        for (path, source) in files {
            let extraction = devmap_extract::extract_file(path, source);
            let callables = extraction
                .symbols
                .iter()
                .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
                .map(|s| Callable {
                    start: s.span.start_byte,
                    end: s.span.end_byte,
                    id: s.qualified_name.clone(),
                    is_method: s.kind == SymbolKind::Method,
                })
                .collect();
            by_file.insert(path.clone(), callables);
        }
        Self { by_file }
    }

    fn innermost(&self, file: &str, byte: usize) -> Option<&Callable> {
        self.by_file
            .get(file)?
            .iter()
            .filter(|c| c.start <= byte && byte < c.end)
            .min_by_key(|c| c.end - c.start)
    }

    /// The id of the function a site at `byte` is written in, or the file's
    /// own path when none contains it (a `static` initializer, a macro item).
    pub fn caller_at(&self, file: &str, byte: usize) -> String {
        self.innermost(file, byte)
            .map_or_else(|| file.to_string(), |c| c.id.clone())
    }

    /// The function whose span holds `byte`. The caller has already checked
    /// that `byte` is a declaration's name; a span alone would also contain a
    /// parameter or a local.
    pub fn target_at(&self, file: &str, byte: usize) -> Option<Target> {
        self.innermost(file, byte).map(|c| Target {
            id: c.id.clone(),
            is_method: c.is_method,
        })
    }

    /// Every id a covered-caller list can name: each file and each callable.
    pub fn callers(&self) -> impl Iterator<Item = &str> {
        self.by_file.iter().flat_map(|(file, callables)| {
            std::iter::once(file.as_str()).chain(callables.iter().map(|c| c.id.as_str()))
        })
    }

    /// The callers a site at `byte` of `file` belongs to: its function and the
    /// file, for every callable the file holds. Used to uncover a whole file.
    pub fn callers_in(&self, file: &str) -> Vec<&str> {
        self.by_file.get(file).map_or_else(Vec::new, |callables| {
            callables.iter().map(|c| c.id.as_str()).collect()
        })
    }
}
