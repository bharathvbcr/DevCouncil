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
//! 5. A macro invoked in item or statement position can declare items and `use`s
//!    in the scope that holds it, and the index cannot expand it.
//!
//! The walk outward from a call site asks, at each scope in turn:
//!
//! * (a) does it declare the name, or import it by name? That answers.
//! * (c) otherwise, does it hold a macro invocation that could emit the name?
//!   Then the answer is `Unknown` and the walk stops. [`macro_providers`] lists the
//!   only macros taken to emit nothing, and only while nothing in the crate
//!   could have rebound the name or its path (see [`CrateMacros`]). An attribute
//!   macro on a block item, or on a `use`, `mod` or `extern crate` item, counts
//!   as a macro in the scope that holds it.
//! * (b) otherwise, does it hold a glob import the index cannot prove does NOT
//!   provide the name? In a block that is `Unknown` and the walk stops; at the
//!   module level the glob is `Glob` and the file-wide bindings say whether it
//!   bound the name. A glob is provably irrelevant only when its source module
//!   is indexed, parsed cleanly, holds no item macro and no private glob, and
//!   neither it nor the globs it re-exports publish the name
//!   ([`Resolver::compute_rust_glob_names`]).
//! * (d) only then does the walk continue outward.
//!
//! Every "could not prove" is `Unknown`; none becomes an outer or a default
//! origin.
//!
//! The extractor files an inline module's items, and the items of a function
//! body, under the file's own namespace, so the symbol table cannot say any of
//! this. `Extraction::rust_scope_facts` records where each module body and
//! item-holding block sits; every declaration and import is filed by the
//! innermost one containing its first byte, and a call site asks which scope it
//! is in. A file whose facts are absent (an extraction written before they
//! existed, or a walk cut short) is `Unknown`, and Unknown never yields a
//! confident edge.

use std::collections::{BTreeMap, BTreeSet};

use devmap_extract::model::{Extraction, RustScopeKind, SymbolKind};

use super::Resolver;

/// The crates whose macro of this name is the real one, for the macros taken to
/// emit no item and no `use` (and, for `thread_local!`, none but the `static`s it
/// names). The one place the list lives. `None` for every other macro.
fn macro_providers(name: &str) -> Option<&'static [&'static str]> {
    const STD: &[&str] = &["std", "core", "alloc"];
    const LOG: &[&str] = &["log", "tracing"];
    const ERR: &[&str] = &["anyhow", "eyre"];
    match name {
        "print" | "println" | "eprint" | "eprintln" | "format" | "format_args" | "write"
        | "writeln" | "vec" | "assert" | "assert_eq" | "assert_ne" | "debug_assert"
        | "debug_assert_eq" | "debug_assert_ne" | "matches" | "panic" | "todo"
        | "unimplemented" | "unreachable" | "dbg" | "thread_local" => Some(STD),
        "trace" | "debug" | "info" | "warn" | "error" => Some(LOG),
        "event" => Some(&["tracing"]),
        "bail" | "ensure" => Some(ERR),
        _ => None,
    }
}

/// Whether `name` is a macro [`macro_providers`] lists.
pub(super) fn is_silent_macro_name(name: &str) -> bool {
    macro_providers(name).is_some()
}

/// Whether the crate named `root` is one that owns the macro `name`.
pub(super) fn macro_owned_by(name: &str, root: &str) -> bool {
    macro_providers(name).is_some_and(|providers| providers.contains(&root))
}

/// What could rebind a macro name inside one crate. A crate whose files do not
/// all record their macros is `complete: false`, and no macro of it is silent.
#[derive(Debug, Clone, Default)]
pub(super) struct CrateMacros {
    pub(super) complete: bool,
    /// Every `macro_rules!` name of the crate.
    pub(super) defined: BTreeSet<String>,
    /// Names a `use` imports from anywhere but the one crate that owns the macro.
    pub(super) rebound: BTreeSet<String>,
    /// Path roots that are not the crate they look like: an `extern crate .. as`
    /// alias, a module of the crate, a `use .. as` of the name.
    pub(super) shadowed_roots: BTreeSet<String>,
    /// A `#[macro_use]` sits somewhere in the crate.
    pub(super) macro_use: bool,
}

/// The crate a file belongs to, as the path before its first `src`, `tests`,
/// `benches` or `examples` directory.
pub(super) fn crate_key(file: &str) -> &str {
    ["/src/", "/tests/", "/benches/", "/examples/"]
        .iter()
        .filter_map(|dir| file.find(dir))
        .min()
        .map_or("", |at| &file[..at])
}

/// Whether the macro invoked as `path` is the real std/log/tracing/anyhow macro
/// and emits nothing the index cannot see. Nothing in the crate may rebind its
/// name or its path root.
fn macro_is_silent(path: &str, env: Option<&CrateMacros>) -> bool {
    let Some(env) = env.filter(|env| env.complete && !env.macro_use) else {
        return false;
    };
    let mut segments: Vec<&str> = path.split("::").map(str::trim).collect();
    let Some(name) = segments.pop() else {
        return false;
    };
    let Some(providers) = macro_providers(name) else {
        return false;
    };
    let path_ok = match segments.as_slice() {
        [] => true,
        [root] => providers.contains(root) && !env.shadowed_roots.contains(*root),
        _ => false,
    };
    path_ok && !env.defined.contains(name) && !env.rebound.contains(name)
}

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
    /// Neither, but a glob import is in the site's module and may bring the
    /// name in.
    Glob,
    /// Nothing in scope binds the name.
    Absent,
    /// The scopes of the file are not known, or are known but not enough to say:
    /// a macro or a glob nearer than any answer may provide the name.
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
    /// Start byte of the import, the key of what it provides in the resolver.
    at: usize,
}

#[derive(Debug, Clone)]
struct Scope {
    module: bool,
    name: String,
    start: usize,
    end: usize,
    parent: Option<usize>,
}

/// The names a block glob might provide, each with the one declaration it comes
/// from (`None` when that is not one declaration the index names).
pub(super) type GlobNames = BTreeMap<String, Option<(String, String)>>;

/// What the index can say about one glob import and one name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GlobAnswer {
    /// The glob provably cannot provide the name.
    Irrelevant,
    /// The glob provably provides it, from one declaration.
    Provides,
    /// It may, or the index cannot say.
    Unknown,
}

/// What a lookup found.
pub(super) struct Lookup {
    pub(super) origin: RustOrigin,
    /// The block glob that provably provides the name, when that is the answer.
    pub(super) glob_at: Option<usize>,
    /// `(import start, local name)` of the named import that answered.
    pub(super) import_at: Option<(usize, String)>,
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
    /// Scopes that hold a macro invocation which may emit items or `use`s.
    macro_scopes: BTreeSet<usize>,
    /// Names the file re-exports from elsewhere (`pub use a::b;`,
    /// `pub use a::b as c;`); its own `pub` items are in `decls`.
    exports: BTreeSet<String>,
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
    /// `macros` is what could rebind a macro name in the file's crate, `None`
    /// when that is not known.
    pub(super) fn build(ext: &Extraction, macros: Option<&CrateMacros>) -> Self {
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
        // Facts written before macros were recorded cannot say which scopes
        // hold one, which is not the same as holding none.
        let Some(macro_facts) = facts.macros.as_ref() else {
            return scope;
        };
        scope.known = true;
        scope.cfg_spans = facts.cfg_spans.clone();
        scope.exports = ext
            .exports
            .iter()
            .filter(|export| {
                export.module_specifier.is_some()
                    && export.exported_name != "*"
                    && !export.exported_name.is_empty()
            })
            .map(|export| export.exported_name.clone())
            .collect();
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
        let mut declared_by_macro: Vec<(usize, String)> = Vec::new();
        for call in &macro_facts.calls {
            if macro_is_silent(&call.name, macros) {
                let at = scope.scope_at(call.start);
                declared_by_macro.extend(call.declares.iter().map(|n| (at, n.clone())));
            } else {
                let at = scope.scope_at(call.start);
                scope.macro_scopes.insert(at);
            }
        }
        // An attribute macro can emit items and `use`s beside its item.
        for attr in &macro_facts.attrs {
            let at = scope.scope_at(attr.start);
            scope.macro_scopes.insert(at);
        }
        for (at, name) in declared_by_macro {
            scope.decls.entry(name).or_default().push(RustDecl {
                scope: at,
                vis: RustVis::Private,
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
                    at: imp.span.start_byte,
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

    /// Whether a module `level` reaches, through `use super::*`, a scope that
    /// holds a macro invocation which may emit the name.
    fn super_levels_hold_macro(&self, level: usize, depth: usize) -> bool {
        if depth >= 8 {
            return true;
        }
        self.globs
            .iter()
            .filter(|g| g.scope == level)
            .filter_map(|g| g.in_file_supers)
            .filter_map(|n| self.climb(level, n))
            .any(|target| {
                self.macro_scopes.contains(&target)
                    || self.super_levels_hold_macro(target, depth + 1)
            })
    }

    /// Whether the glob import that starts at `at` is the one of a block that
    /// sits in the file's own module: the only globs whose source module
    /// [`Resolver::compute_rust_glob_names`] can name.
    pub(super) fn is_root_block_glob(&self, at: usize) -> bool {
        let scope = self.scope_at(at);
        !self.scopes[scope].module && self.module_of(scope) == 0
    }

    /// Whether `at` starts a glob import that names another file.
    pub(super) fn has_cross_file_glob_at(&self, at: usize) -> bool {
        self.globs
            .iter()
            .any(|g| g.at == at && g.in_file_supers.is_none())
    }

    pub(super) fn exports(&self) -> &BTreeSet<String> {
        &self.exports
    }

    pub(super) fn imported_names(&self) -> impl Iterator<Item = &String> {
        self.named.keys()
    }

    /// Where `name` written at `pos` comes from. `glob_answer(at)` says what the
    /// index can prove of the glob import starting at `at` and this name.
    pub(super) fn lookup(
        &self,
        name: &str,
        pos: usize,
        glob_answer: &dyn Fn(usize) -> GlobAnswer,
    ) -> Lookup {
        let unknown = Lookup {
            origin: RustOrigin::Unknown,
            import_at: None,
            glob_at: None,
        };
        if !self.known {
            return unknown;
        }
        let mut level = self.scope_at(pos);
        let mut glob_in_module = false;
        loop {
            // (a) A declaration or a named import answers.
            if let Some((origin, import_at)) = self.explicit_in(level, name, 0) {
                return Lookup {
                    origin,
                    import_at,
                    glob_at: None,
                };
            }
            // (c) A macro that may emit the name leaves it unanswered here.
            if self.macro_scopes.contains(&level)
                || (self.scopes[level].module && self.super_levels_hold_macro(level, 0))
            {
                return unknown;
            }
            // (b) A glob that may provide it does too.
            let module = self.scopes[level].module;
            let mut unproven = false;
            let mut provides: Vec<usize> = Vec::new();
            for g in self.globs.iter().filter(|g| g.scope == level) {
                match g.in_file_supers {
                    // A module follows `use super::*` through its parent.
                    Some(_) => unproven |= !module,
                    None => match glob_answer(g.at) {
                        GlobAnswer::Irrelevant => {}
                        GlobAnswer::Provides => provides.push(g.at),
                        GlobAnswer::Unknown => unproven = true,
                    },
                }
            }
            if !module {
                // Two globs that both provide the name make it ambiguous.
                if unproven || provides.len() > 1 {
                    return unknown;
                }
                if let Some(&at) = provides.first() {
                    return Lookup {
                        origin: RustOrigin::Glob,
                        import_at: None,
                        glob_at: Some(at),
                    };
                }
            } else if unproven || !provides.is_empty() {
                glob_in_module = true;
            }
            // (d) Outward, up to the module boundary.
            if module {
                break;
            }
            match self.scopes[level].parent {
                Some(parent) => level = parent,
                None => break,
            }
        }
        Lookup {
            origin: if glob_in_module {
                RustOrigin::Glob
            } else {
                RustOrigin::Absent
            },
            import_at: None,
            glob_at: None,
        }
    }
}

impl Resolver {
    /// The lookup of `name` at `pos` in `file`, with what the index can prove of the
    /// glob imports in scope. `None` for a file this resolver
    /// holds no Rust scope for.
    fn rust_lookup(&self, file: &str, name: &str, pos: usize) -> Option<Lookup> {
        let scope = self.rust_site_scope.get(file)?;
        Some(scope.lookup(
            name,
            pos,
            &|at| match self.rust_glob_names.get(&(file.to_string(), at)) {
                None => GlobAnswer::Unknown,
                Some(provided) => match provided.get(name) {
                    None => GlobAnswer::Irrelevant,
                    Some(Some(_)) => GlobAnswer::Provides,
                    Some(None) => GlobAnswer::Unknown,
                },
            },
        ))
    }

    /// Where the bare `name`, written at `site` in `file`, comes from. `None`
    /// for a file this resolver holds no Rust scope for.
    pub(super) fn rust_origin_at(&self, file: &str, name: &str, site: Site) -> Option<RustOrigin> {
        Some(self.rust_lookup(file, name, site.pos)?.origin)
    }

    /// The `(file, declared name)` the explicit import that answers `name` at
    /// `site` binds, when the answer is a named import.
    pub(super) fn rust_named_binding_at(
        &self,
        file: &str,
        name: &str,
        site: Site,
    ) -> Option<&(String, String)> {
        let lookup = self.rust_lookup(file, name, site.pos)?;
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
        let lookup = self.rust_lookup(file, name, site.pos)?;
        match lookup.origin {
            RustOrigin::Named => self.rust_named_binding_at(file, name, site),
            // A block glob the index proved to provide the name binds it to its
            // own declaration; any other glob is told by the file-wide table.
            RustOrigin::Glob if lookup.glob_at.is_some() => self
                .rust_glob_names
                .get(&(file.to_string(), lookup.glob_at?))?
                .get(name)?
                .as_ref(),
            RustOrigin::Glob | RustOrigin::Absent => self.import_bindings.get(file)?.get(name),
            RustOrigin::Own | RustOrigin::Unknown => None,
        }
    }

    /// Whether the corpus-unique rung must not bind a bare Rust call to a
    /// declaration of `target_file`: the origin of the name is unknown here, and
    /// the one namesake lies in the call's own file, which is the declaration a
    /// nearer macro or glob would be shadowing. A namesake in another file is
    /// the unique rung's ordinary guess and stays at its tier.
    pub(super) fn rust_unique_rung_declines(
        &self,
        family: super::LangFamily,
        file: &str,
        name: &str,
        target_file: &str,
        site: Site,
    ) -> bool {
        family == super::LangFamily::Rust
            && target_file == file
            && self.rust_origin_at(file, name, site) == Some(RustOrigin::Unknown)
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
