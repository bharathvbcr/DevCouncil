//! Whether a Rust name is in scope at one call site.
//!
//! The model, as far as this resolver needs it (the Rust reference's name
//! resolution, reduced):
//!
//! 1. A module - a file, an inline `mod`, a `#[path]` child - is a namespace
//!    boundary. Code in it sees the names declared or imported IN THAT MODULE
//!    and the preludes. It does not see its parent's, ancestors' or siblings'
//!    names; those need a path (`super::x`, `crate::..`) or an import
//!    (`use super::*`, which brings the parent's names in).
//! 2. A block (a function body, a nested block, a closure body) is a scope
//!    inside its module. Items and `use`s written in a block are visible in that
//!    block and the blocks nested in it, whatever their order, and shadow the
//!    enclosing scopes' names. The walk outward stops at the module boundary.
//! 3. In one scope, a declared item or a named import shadows a glob import. Two
//!    globs that provide one name make it ambiguous, so a reader that cannot
//!    see every publisher must not pick one confidently.
//! 4. A glob publishes only what is visible to the importing module: private
//!    items to their own module and its descendants, `pub(super)` to the parent
//!    module and its descendants, `pub(in path)` to that module and its
//!    descendants, `pub(crate)` and `pub` to everyone.
//!
//! The extractor files an inline module's items, and the items of a function
//! body, under the file's own namespace, so the symbol table cannot say any of
//! this. `Extraction::rust_scope_facts` records where each module body and
//! item-holding block sits; every declaration and import is filed by the
//! innermost one containing its first byte, and a call site asks which scope it
//! is in. A file whose facts are absent (an extraction written before they
//! existed, or a walk cut short) is `Unknown`, and Unknown never yields a
//! confident edge.

use std::collections::BTreeMap;

use devmap_extract::model::{Extraction, RustScopeKind, SymbolKind};

use super::Resolver;

/// Where a use of a name inside a file comes from, as far as that file's own
/// scopes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RustOrigin {
    /// A declaration in scope: in the site's block, an enclosing block, or the
    /// site's module (including one reached through `use super::*` or a
    /// `use` path that names an item of this same file).
    Own,
    /// An explicit `use path::Name;` in scope that names another file.
    Named,
    /// Neither, but a glob import is in scope and may bring the name in.
    Glob,
    /// Nothing in scope binds the name.
    Absent,
    /// The scopes of the file are not known, or are known but not enough to say.
    Unknown,
}

/// Where a name is used: a byte offset in the file.
#[derive(Debug, Clone, Copy)]
pub(super) struct Site {
    pub pos: usize,
}

impl Site {
    pub(super) fn of(pos: usize) -> Self {
        Self { pos }
    }
}

/// The visibility of an item to the module that wants to import it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RustVis {
    /// Private or `pub(self)`.
    Private,
    /// `pub(super)`.
    Super,
    /// `pub(in path)`, the path as written.
    In(String),
    /// `pub(crate)` or `pub`.
    Open,
}

#[derive(Debug, Clone)]
pub(super) struct RustDecl {
    scope: usize,
    pub(super) vis: RustVis,
}

#[derive(Debug, Clone)]
struct NamedImport {
    scope: usize,
    /// Start byte of the import, the key of its binding in the resolver.
    at: usize,
    imported: String,
    /// The scope this path names when it stays inside the file.
    in_file_target: Option<usize>,
}

#[derive(Debug, Clone)]
struct GlobImport {
    scope: usize,
    /// Enclosing modules climbed by `use super::*` that stay inside the file;
    /// `None` for a glob that names another file.
    in_file_supers: Option<usize>,
}

#[derive(Debug, Clone)]
struct Scope {
    module: bool,
    name: String,
    start: usize,
    end: usize,
    parent: Option<usize>,
}

/// What a lookup found.
pub(super) struct Lookup {
    pub(super) origin: RustOrigin,
    /// `(import start, local name)` of the named import that answered.
    pub(super) import_at: Option<(usize, String)>,
    /// A glob sits in a scope nearer than the one that answered.
    pub(super) glob_nearer: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct RustFileScope {
    known: bool,
    /// Index 0 is the file's own module.
    scopes: Vec<Scope>,
    decls: BTreeMap<String, Vec<RustDecl>>,
    named: BTreeMap<String, Vec<NamedImport>>,
    globs: Vec<GlobImport>,
    cfg_spans: Vec<(usize, usize)>,
}

fn leading_supers(raw_import: &str) -> usize {
    let text = raw_import.trim();
    let text = text
        .split_once("use ")
        .map_or(text, |(_, rest)| rest)
        .trim_start();
    text.split("::")
        .take_while(|segment| segment.trim() == "super")
        .count()
}

impl RustFileScope {
    pub(super) fn build(ext: &Extraction) -> Self {
        let mut scope = Self {
            scopes: vec![Scope {
                module: true,
                name: String::new(),
                start: 0,
                end: usize::MAX,
                parent: None,
            }],
            ..Self::default()
        };
        let Some(facts) = ext.rust_scope_facts.as_ref() else {
            return scope;
        };
        scope.known = true;
        scope.cfg_spans = facts.cfg_spans.clone();
        // `facts.scopes` is in source order, so a scope's parent is the nearest
        // earlier one that contains it.
        for span in &facts.scopes {
            let parent = (0..scope.scopes.len())
                .rev()
                .find(|&i| scope.scopes[i].start <= span.start && span.end <= scope.scopes[i].end);
            scope.scopes.push(Scope {
                module: span.kind == RustScopeKind::Module,
                name: span.name.clone(),
                start: span.start,
                end: span.end,
                parent,
            });
        }
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
            // A member of a type or trait is reached through it, never by its
            // bare name. Anything else filed under a function sits in a block.
            if let Some((parent, _)) = within.rsplit_once('.') {
                let parent_qualified = format!("{file}::{parent}");
                if !matches!(
                    kinds.get(parent_qualified.as_str()),
                    Some(SymbolKind::Function | SymbolKind::Method)
                ) {
                    continue;
                }
            }
            let vis = if !sym.is_exported {
                RustVis::Private
            } else {
                match facts
                    .restricted_visibility
                    .iter()
                    .find(|(at, _)| *at == sym.span.start_byte)
                {
                    None => RustVis::Open,
                    Some((_, text)) if text == "pub(crate)" => RustVis::Open,
                    Some((_, text)) if text == "pub(super)" => RustVis::Super,
                    Some((_, text)) if text == "pub(self)" => RustVis::Private,
                    Some((_, text)) => text
                        .strip_prefix("pub(in")
                        .and_then(|rest| rest.strip_suffix(')'))
                        .map_or(RustVis::Private, |path| RustVis::In(path.to_string())),
                }
            };
            let at = scope.scope_at(sym.span.start_byte);
            scope
                .decls
                .entry(sym.name.clone())
                .or_default()
                .push(RustDecl { scope: at, vis });
        }
        for imp in &ext.imports {
            if imp.raw_import.trim_start().starts_with("mod ") {
                continue;
            }
            let at = scope.scope_at(imp.span.start_byte);
            let supers = leading_supers(&imp.raw_import);
            let climbed = scope.climb(scope.module_of(at), supers);
            if imp.alias.as_deref() == Some(".") {
                let first = imp.module_specifier.split("::").next().unwrap_or("");
                let in_file_supers = (supers > 0
                    && first == "self"
                    && imp.module_specifier == "self"
                    && climbed.is_some())
                .then_some(supers);
                scope.globs.push(GlobImport {
                    scope: at,
                    in_file_supers,
                });
                continue;
            }
            for (i, imported) in imp.imported_names.iter().enumerate() {
                let local = imp
                    .local_names
                    .get(i)
                    .filter(|local| !local.is_empty())
                    .cloned()
                    .or_else(|| {
                        imp.alias
                            .clone()
                            .filter(|a| imp.imported_names.len() == 1 && a != "." && a != "_")
                    })
                    .unwrap_or_else(|| imported.clone());
                let in_file_target = scope.resolve_in_file(at, supers, &imp.module_specifier);
                scope.named.entry(local).or_default().push(NamedImport {
                    scope: at,
                    at: imp.span.start_byte,
                    imported: imported.clone(),
                    in_file_target,
                });
            }
        }
        scope
    }

    /// The innermost scope whose body contains `pos`; 0 is the file itself.
    fn scope_at(&self, pos: usize) -> usize {
        (0..self.scopes.len())
            .rev()
            .find(|&i| i == 0 || (self.scopes[i].start <= pos && pos < self.scopes[i].end))
            .unwrap_or(0)
    }

    /// The module that holds `scope`: itself when it is one.
    fn module_of(&self, mut scope: usize) -> usize {
        while !self.scopes[scope].module {
            match self.scopes[scope].parent {
                Some(parent) => scope = parent,
                None => return 0,
            }
        }
        scope
    }

    /// `module` climbed `n` modules out, when that stays inside the file.
    fn climb(&self, mut module: usize, n: usize) -> Option<usize> {
        for _ in 0..n {
            module = self.module_of(self.scopes[module].parent?);
        }
        Some(module)
    }

    fn child_module(&self, parent_module: usize, name: &str) -> Option<usize> {
        (1..self.scopes.len()).find(|&i| {
            self.scopes[i].module
                && self.scopes[i].name == name
                && self.scopes[i]
                    .parent
                    .is_some_and(|p| self.module_of(p) == parent_module)
        })
    }

    /// The module a `use` path names when every segment is an inline module of
    /// this file; `None` when the path leaves the file or cannot be followed.
    fn resolve_in_file(&self, at: usize, supers: usize, specifier: &str) -> Option<usize> {
        let module = self.module_of(at);
        let mut segments = specifier.split("::");
        let first = segments.next()?;
        let mut current = match first {
            "self" => self.climb(module, supers)?,
            "super" | "crate" | "" => return None,
            name => self.child_module(module, name).or_else(|| {
                // `use super::*; use checkout::pay;` - the glob brings the
                // parent's module names in.
                self.globs
                    .iter()
                    .filter(|g| self.module_of(g.scope) == module)
                    .filter_map(|g| g.in_file_supers)
                    .find_map(|n| {
                        let target = self.climb(module, n)?;
                        self.child_module(target, name)
                    })
            })?,
        };
        for segment in segments {
            current = self.child_module(current, segment)?;
        }
        Some(current)
    }

    /// Whether the file declares `name` anywhere at all, at any depth.
    pub(super) fn declares_anywhere(&self, name: &str) -> bool {
        self.decls.contains_key(name)
    }

    pub(super) fn is_known(&self) -> bool {
        self.known
    }

    /// Whether the start byte of a function or impl sits under `#[cfg]`.
    pub(super) fn is_under_cfg(&self, start: usize) -> bool {
        self.cfg_spans
            .iter()
            .any(|(from, to)| *from <= start && start < *to)
    }

    /// The declaration of `name` made directly in the file's own module, which
    /// is what a glob import of the file can publish.
    pub(super) fn root_decl(&self, name: &str) -> Option<&RustDecl> {
        self.decls.get(name)?.iter().find(|decl| decl.scope == 0)
    }

    /// Whether `name` is declared or imported by name in the same scope as
    /// `pos`. Either shadows a glob written there.
    pub(super) fn shadows_glob_at(&self, name: &str, pos: usize) -> bool {
        let scope = self.scope_at(pos);
        self.decls
            .get(name)
            .is_some_and(|decls| decls.iter().any(|d| d.scope == scope))
            || self
                .named
                .get(name)
                .is_some_and(|named| named.iter().any(|n| n.scope == scope))
    }

    fn explicit_in(
        &self,
        level: usize,
        name: &str,
        depth: usize,
    ) -> Option<(RustOrigin, Option<(usize, String)>)> {
        if self
            .decls
            .get(name)
            .is_some_and(|decls| decls.iter().any(|d| d.scope == level))
        {
            return Some((RustOrigin::Own, None));
        }
        if let Some(import) = self
            .named
            .get(name)
            .and_then(|named| named.iter().find(|n| n.scope == level))
        {
            let in_file = import.in_file_target.is_some_and(|target| {
                import.imported == name
                    && self
                        .decls
                        .get(&import.imported)
                        .is_some_and(|decls| decls.iter().any(|d| d.scope == target))
            });
            return Some(if in_file {
                (RustOrigin::Own, None)
            } else {
                (RustOrigin::Named, Some((import.at, name.to_string())))
            });
        }
        // A module that writes `use super::*;` has its parent's names.
        if self.scopes[level].module && depth < 8 {
            for glob in self.globs.iter().filter(|g| g.scope == level) {
                if let Some(n) = glob.in_file_supers {
                    if let Some(target) = self.climb(level, n) {
                        if let Some(found) = self.explicit_in(target, name, depth + 1) {
                            return Some(found);
                        }
                    }
                }
            }
        }
        None
    }

    pub(super) fn lookup(&self, name: &str, pos: usize) -> Lookup {
        if !self.known {
            return Lookup {
                origin: RustOrigin::Unknown,
                import_at: None,
                glob_nearer: false,
            };
        }
        let mut level = self.scope_at(pos);
        let mut glob_nearer = false;
        loop {
            if let Some((origin, import_at)) = self.explicit_in(level, name, 0) {
                return Lookup {
                    origin,
                    import_at,
                    glob_nearer,
                };
            }
            if self
                .globs
                .iter()
                .any(|g| g.scope == level && g.in_file_supers.is_none())
            {
                glob_nearer = true;
            }
            if self.scopes[level].module {
                break;
            }
            match self.scopes[level].parent {
                Some(parent) => level = parent,
                None => break,
            }
        }
        Lookup {
            origin: if glob_nearer {
                RustOrigin::Glob
            } else {
                RustOrigin::Absent
            },
            import_at: None,
            glob_nearer: false,
        }
    }
}

impl Resolver {
    /// Where the bare `name`, written at `site` in `file`, comes from. `None`
    /// for a file this resolver holds no Rust scope for.
    pub(super) fn rust_origin_at(&self, file: &str, name: &str, site: Site) -> Option<RustOrigin> {
        let lookup = self.rust_site_scope.get(file)?.lookup(name, site.pos);
        // A glob in a scope nearer than the answer shadows it only if that glob
        // provides the name, which the file-wide bindings say.
        if lookup.glob_nearer
            && lookup.origin != RustOrigin::Unknown
            && self
                .import_bindings
                .get(file)
                .is_some_and(|bindings| bindings.contains_key(name))
        {
            return Some(RustOrigin::Glob);
        }
        Some(lookup.origin)
    }

    /// The `(file, declared name)` the explicit import that answers `name` at
    /// `site` binds, when the answer is a named import.
    pub(super) fn rust_named_binding_at(
        &self,
        file: &str,
        name: &str,
        site: Site,
    ) -> Option<&(String, String)> {
        let lookup = self.rust_site_scope.get(file)?.lookup(name, site.pos);
        if lookup.origin != RustOrigin::Named {
            return None;
        }
        let (at, local) = lookup.import_at?;
        self.rust_import_targets.get(&(file.to_string(), at, local))
    }

    /// The `(file, declared name)` an import binds `name` to where it is read at
    /// `site`: Rust asks its scopes - an explicit `use` answers with its own
    /// target, a glob or an unbound name with the file-wide table - and every
    /// other family asks [`Self::import_binding_at`].
    pub(super) fn import_binding_for_site(
        &self,
        family: super::LangFamily,
        file: &str,
        scope: Option<&str>,
        name: &str,
        site: Site,
    ) -> Option<&(String, String)> {
        if family != super::LangFamily::Rust {
            return self.import_binding_at(family, file, scope, name);
        }
        match self.rust_origin_at(file, name, site)? {
            RustOrigin::Named => self.rust_named_binding_at(file, name, site),
            RustOrigin::Glob | RustOrigin::Absent => self.import_bindings.get(file)?.get(name),
            RustOrigin::Own | RustOrigin::Unknown => None,
        }
    }

    /// Whether the import rung must stand aside for a bare Rust call: a
    /// declaration in scope shadows a glob, and a name whose binding cannot be
    /// told apart is declined rather than guessed. Every other family answers
    /// `false`.
    pub(super) fn rust_import_rung_declines(
        &self,
        family: super::LangFamily,
        file: &str,
        name: &str,
        site: Site,
    ) -> bool {
        family == super::LangFamily::Rust
            && matches!(
                self.rust_origin_at(file, name, site),
                Some(RustOrigin::Own | RustOrigin::Unknown)
            )
    }

    /// Whether the same-file rung must stand aside for a bare Rust call: it may
    /// bind only a declaration that is in scope at the site, so a name an import
    /// brings in, or one declared in a scope the site cannot see, is not its to
    /// answer. Every other family answers `false`.
    pub(super) fn rust_same_file_rung_declines(
        &self,
        family: super::LangFamily,
        file: &str,
        name: &str,
        site: Site,
    ) -> bool {
        family == super::LangFamily::Rust
            && self
                .rust_origin_at(file, name, site)
                .is_some_and(|origin| origin != RustOrigin::Own)
    }
}
