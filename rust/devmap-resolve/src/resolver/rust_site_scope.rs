//! Whether a Rust name is in scope at one call site.
//!
//! Rust scope is the module - an inline `mod name { .. }` block or a function
//! body included - not the file. The extractor files an inline module's items
//! under the file's own namespace, so "this file declares `Dup`" says nothing
//! about whether the code asking for `Dup` can see that declaration. Every
//! rung that narrows a Rust name by "the call file declares it" asks this
//! module instead, so there is one answer to one question.
//!
//! The answer is built from byte spans: the inline-module blocks the extractor
//! recorded (`Extraction::rust_module_spans`), and the spans of each
//! declaration, import and call. A site sees the declarations and imports of
//! its own module and of the modules around it (an ancestor's names reach a
//! child through `use super::*;`, which a compiling program must have written);
//! it never sees a sibling's or a descendant's. A declaration inside a function
//! body is visible only inside that function.

use std::collections::BTreeMap;

use devmap_extract::model::{Extraction, SymbolKind};

use super::Resolver;

/// Where a name is used: a byte offset in the file and the callable that
/// encloses it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Site<'a> {
    pub pos: usize,
    pub caller: Option<&'a str>,
}

impl<'a> Site<'a> {
    pub(super) fn of(pos: usize, caller: Option<&'a str>) -> Self {
        Self { pos, caller }
    }
}

/// Where a bare Rust name comes from at one call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RustOrigin {
    /// A declaration in the site's own module, an enclosing one, or the
    /// enclosing function. It shadows anything a glob brings in.
    Own,
    /// An explicit `use path::Name;` in the site's module or an enclosing one.
    Named,
    /// Neither, but a glob import is in scope and may bring the name in.
    Glob,
    /// Nothing in scope binds the name.
    Absent,
    /// The file has inline modules whose extents were not recorded (an
    /// extraction written before they were) and more than one kind of binding
    /// for the name, so which one the site sees cannot be told.
    Unknown,
}

/// One declaration of a bare name that a site could see.
#[derive(Debug, Clone)]
struct Decl {
    /// Index into [`RustFileScope::modules`]; `None` is the file's own level.
    module: Option<usize>,
    /// The qualified name of the function the declaration sits inside, or
    /// `None` for a module-level item.
    owner: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct RustFileScope {
    /// `(start, end, parent)` of each inline module, in source order.
    modules: Vec<(usize, usize, Option<usize>)>,
    /// Inline modules exist but their extents were not collected.
    extents_unknown: bool,
    decls: BTreeMap<String, Vec<Decl>>,
    named: BTreeMap<String, Vec<Option<usize>>>,
    globs: Vec<Option<usize>>,
}

impl RustFileScope {
    pub(super) fn build(ext: &Extraction) -> Self {
        let mut spans: Vec<(usize, usize)> = ext
            .rust_module_spans
            .iter()
            .map(|(_, start, end)| (*start, *end))
            .collect();
        spans.sort();
        // A module's parent is the innermost earlier module that contains it.
        let mut modules: Vec<(usize, usize, Option<usize>)> = Vec::with_capacity(spans.len());
        for (start, end) in spans {
            let parent = (0..modules.len())
                .rev()
                .find(|&i| modules[i].0 <= start && end <= modules[i].1);
            modules.push((start, end, parent));
        }
        let mut scope = Self {
            modules,
            extents_unknown: !ext.rust_inline_modules.is_empty()
                && ext.rust_module_spans.is_empty(),
            ..Self::default()
        };
        let kinds: BTreeMap<&str, SymbolKind> = ext
            .symbols
            .iter()
            .map(|sym| (sym.qualified_name.as_str(), sym.kind))
            .collect();
        for sym in &ext.symbols {
            if matches!(
                sym.kind,
                SymbolKind::File | SymbolKind::Module | SymbolKind::Field
            ) {
                continue;
            }
            let Some((file, within)) = sym.qualified_name.split_once("::") else {
                continue;
            };
            let owner = match within.rsplit_once('.') {
                None => None,
                Some((parent, _)) => {
                    let parent_qualified = format!("{file}::{parent}");
                    match kinds.get(parent_qualified.as_str()) {
                        Some(SymbolKind::Function | SymbolKind::Method) => Some(parent.to_string()),
                        // A member of a type or trait is reached through it,
                        // never by its bare name.
                        _ => continue,
                    }
                }
            };
            let module = scope.module_at(sym.span.start_byte);
            scope
                .decls
                .entry(sym.name.clone())
                .or_default()
                .push(Decl { module, owner });
        }
        for imp in &ext.imports {
            let module = scope.module_at(imp.span.start_byte);
            if imp.alias.as_deref() == Some(".") {
                scope.globs.push(module);
                continue;
            }
            let names = if imp.local_names.is_empty() {
                &imp.imported_names
            } else {
                &imp.local_names
            };
            for name in names {
                scope.named.entry(name.clone()).or_default().push(module);
            }
            if let Some(alias) = imp.alias.as_deref().filter(|a| *a != "_" && !a.is_empty()) {
                scope
                    .named
                    .entry(alias.to_string())
                    .or_default()
                    .push(module);
            }
        }
        scope
    }

    /// The innermost inline module containing `pos`; `None` is the file level.
    pub(super) fn module_at(&self, pos: usize) -> Option<usize> {
        (0..self.modules.len())
            .rev()
            .find(|&i| self.modules[i].0 <= pos && pos < self.modules[i].1)
    }

    /// `module` and each module around it, innermost first, ending at the file.
    fn chain(&self, module: Option<usize>) -> Vec<Option<usize>> {
        let mut out = vec![module];
        let mut at = module;
        while let Some(i) = at {
            at = self.modules[i].2;
            out.push(at);
        }
        out
    }

    /// Whether the file declares `name` anywhere at all, at any depth.
    pub(super) fn declares_anywhere(&self, name: &str) -> bool {
        self.decls.contains_key(name)
    }

    /// Whether `name` is declared in the same module as `pos`, ignoring
    /// function bodies. This is what shadows a glob written at `pos`.
    pub(super) fn declares_in_module_of(&self, name: &str, pos: usize) -> bool {
        let module = self.module_at(pos);
        self.decls.get(name).is_some_and(|decls| {
            decls
                .iter()
                .any(|d| d.owner.is_none() && d.module == module)
        })
    }

    /// Whether `name` is imported explicitly (`use path::Name;`) in the same
    /// module as `pos`. An explicit import wins over a glob in that module.
    pub(super) fn imports_by_name_in_module_of(&self, name: &str, pos: usize) -> bool {
        let module = self.module_at(pos);
        self.named
            .get(name)
            .is_some_and(|modules| modules.contains(&module))
    }

    pub(super) fn origin(&self, name: &str, pos: usize, caller: Option<&str>) -> RustOrigin {
        let decls = self.decls.get(name);
        let named = self.named.get(name);
        if self.extents_unknown {
            return match (decls.is_some(), named.is_some()) {
                (true, true) => RustOrigin::Unknown,
                (true, false) => RustOrigin::Own,
                (false, true) => RustOrigin::Named,
                (false, false) if !self.globs.is_empty() => RustOrigin::Glob,
                (false, false) => RustOrigin::Absent,
            };
        }
        let chain = self.chain(self.module_at(pos));
        for level in &chain {
            let owns = decls.is_some_and(|decls| {
                decls.iter().any(|d| {
                    d.module == *level
                        && d.owner.as_deref().is_none_or(|owner| {
                            caller.is_some_and(|caller| {
                                let caller = caller.split_once("::").map_or(caller, |(_, c)| c);
                                caller == owner
                                    || caller
                                        .strip_prefix(owner)
                                        .is_some_and(|rest| rest.starts_with('.'))
                            })
                        })
                })
            });
            if owns {
                return RustOrigin::Own;
            }
            if named.is_some_and(|modules| modules.contains(level)) {
                return RustOrigin::Named;
            }
        }
        if self.globs.iter().any(|g| chain.contains(g)) {
            RustOrigin::Glob
        } else {
            RustOrigin::Absent
        }
    }
}

impl Resolver {
    /// Where the bare `name`, written at byte `pos` inside `caller` in `file`,
    /// comes from. `None` for a file this resolver holds no Rust scope for.
    pub(super) fn rust_origin_at(
        &self,
        file: &str,
        name: &str,
        pos: usize,
        caller: Option<&str>,
    ) -> Option<RustOrigin> {
        Some(self.rust_site_scope.get(file)?.origin(name, pos, caller))
    }
}

impl Resolver {
    /// Whether the import rung must stand aside for a bare Rust call: a
    /// declaration in scope shadows a glob, and a name whose binding cannot be
    /// told apart is declined rather than guessed. Every other family answers
    /// `false`.
    pub(super) fn rust_import_rung_declines(
        &self,
        family: super::LangFamily,
        file: &str,
        name: &str,
        pos: usize,
        caller: Option<&str>,
    ) -> bool {
        family == super::LangFamily::Rust
            && matches!(
                self.rust_origin_at(file, name, pos, caller),
                Some(RustOrigin::Own | RustOrigin::Unknown)
            )
    }

    /// Whether the same-file rung must stand aside for a bare Rust call: it
    /// may bind only a declaration that is in scope at the site, so a name an
    /// import brings in, or one declared in a module the site cannot see, is
    /// not its to answer. Every other family answers `false`.
    pub(super) fn rust_same_file_rung_declines(
        &self,
        family: super::LangFamily,
        file: &str,
        name: &str,
        pos: usize,
        caller: Option<&str>,
    ) -> bool {
        family == super::LangFamily::Rust
            && self
                .rust_origin_at(file, name, pos, caller)
                .is_some_and(|origin| origin != RustOrigin::Own)
    }
}
