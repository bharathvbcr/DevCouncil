use super::*;

/// The type names every Kotlin file sees without an import (`kotlin.*`,
/// `kotlin.collections.*`, `kotlin.sequences.*`, `kotlin.text.*`, `kotlin.io.*`
/// and `kotlin.ranges.*`), which a repository class of the same name does not
/// replace unless the file declares or imports it.
const KOTLIN_DEFAULT_IMPORTS: &[&str] = &[
    "Any",
    "Array",
    "ArrayList",
    "BooleanArray",
    "Collection",
    "Comparable",
    "Comparator",
    "Deferred",
    "Entry",
    "Enum",
    "Error",
    "Exception",
    "HashMap",
    "HashSet",
    "Iterable",
    "Iterator",
    "LinkedHashMap",
    "LinkedHashSet",
    "List",
    "ListIterator",
    "Lazy",
    "Map",
    "MutableCollection",
    "MutableIterable",
    "MutableIterator",
    "MutableList",
    "MutableMap",
    "MutableSet",
    "Nothing",
    "Number",
    "Pair",
    "Result",
    "Sequence",
    "Set",
    "String",
    "StringBuilder",
    "Throwable",
    "Triple",
    "Unit",
];

impl Resolver {
    /// The one indexed `Class`/`Struct` called `name` in a family `family`
    /// admits, or `None` when there is no such type or more than one.
    ///
    /// The same test the receiver maps have always applied before binding a
    /// name to a type: a type the corpus declares twice cannot be dispatched
    /// on, so it types nothing.
    pub(super) fn unique_indexed_type(&self, family: LangFamily, name: &str) -> bool {
        self.symbol_index.get(name).is_some_and(|candidates| {
            candidates
                .iter()
                .filter(|(_, kind, candidate_family, _)| {
                    family.admits(*candidate_family)
                        && matches!(kind, SymbolKind::Class | SymbolKind::Struct)
                })
                .count()
                == 1
        })
    }

    /// Fill [`Self::module_value_types`] for one file.
    ///
    /// A module-scope value is typed by its declaration's **initializer** —
    /// the outermost reference the extractor attached to the declared name
    /// inside the declaration's own span. "Outermost" is the earliest start:
    /// in `new Service(makeDep())` the constructor begins before its
    /// arguments, and every argument carries the same `assigned_to`, so taking
    /// any of them would type the singleton by its dependency. A tie at the
    /// earliest offset is two readings of one expression and abstains.
    ///
    /// Withdrawn — never re-answered — when anything else in the file assigns
    /// the name a constructor or call: a second module-level assignment, or a
    /// function that reassigns it (`function swap() { svc = new Other(); }`).
    /// A function-local of the same name is indistinguishable from that
    /// reassignment in the extraction, so it withdraws the answer too; that
    /// costs an abstention, where reading it the other way would cost a
    /// confidently wrong edge.
    pub(super) fn index_module_value_types(&mut self, ext: &Extraction, family: LangFamily) {
        let mut declarations: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
        for symbol in &ext.symbols {
            if symbol.kind == SymbolKind::Variable
                && symbol.parent_symbol.as_deref() == Some(ext.file_path.as_str())
            {
                declarations
                    .entry(symbol.name.as_str())
                    .or_default()
                    .push(&symbol.span);
            }
        }
        // An unexported Go package var is no symbol — the extractor publishes
        // only exported package bindings — but its initializer is bound to its
        // name at package scope. Go has no package-level statements other than
        // declarations, and forbids declaring a name twice in a package, so the
        // whole file is that declaration's span: every package-scope reference
        // bound to the name belongs to it.
        if ext.language == "go" {
            if let Some(file_span) = ext
                .symbols
                .iter()
                .find(|symbol| symbol.kind == SymbolKind::File)
                .map(|symbol| &symbol.span)
            {
                for reference in &ext.references {
                    let Some(bound) = reference.assigned_to.as_deref() else {
                        continue;
                    };
                    if reference.enclosing_symbol.is_none()
                        && matches!(
                            reference.kind,
                            ReferenceKind::Constructor | ReferenceKind::Call
                        )
                        && !declarations.contains_key(bound)
                    {
                        declarations.insert(bound, vec![file_span]);
                    }
                }
            }
        }
        for (name, spans) in declarations {
            // Two module-level declarations of one name: the type is whichever
            // ran last, which is not a fact the extraction holds.
            let [span] = spans.as_slice() else {
                continue;
            };
            let within = |inner: &Span| {
                inner.start_byte >= span.start_byte && inner.end_byte <= span.end_byte
            };
            // What the declaration itself spells: methods written inside it,
            // and names it reads at module scope (`{ get, post }`). A name read
            // inside one of those methods' bodies has an enclosing symbol and
            // is not a member.
            let members: BTreeSet<String> = ext
                .symbols
                .iter()
                .filter(|symbol| symbol.name != name && within(&symbol.span))
                .map(|symbol| symbol.name.clone())
                .chain(
                    ext.references
                        .iter()
                        .filter(|reference| {
                            reference.enclosing_symbol.is_none()
                                && reference.receiver_expr.is_none()
                                && matches!(reference.kind, ReferenceKind::Name)
                                && within(&reference.span)
                        })
                        .map(|reference| reference.name.clone()),
                )
                .collect();
            // Members are read only for a value import, which Go does not have
            // — and a Go declaration's span may be the whole file, which would
            // make every symbol in it a member.
            if !members.is_empty() && ext.language != "go" {
                self.module_value_members
                    .insert((ext.file_path.clone(), name.to_string()), members);
            }
            let mut earliest: Option<(usize, &ExtractedReference)> = None;
            let mut tied = false;
            let mut reassigned = false;
            for reference in &ext.references {
                if reference.assigned_to.as_deref() != Some(name)
                    || !matches!(
                        reference.kind,
                        ReferenceKind::Constructor | ReferenceKind::Call
                    )
                {
                    continue;
                }
                let in_declaration = reference.enclosing_symbol.is_none()
                    && reference.span.start_byte >= span.start_byte
                    && reference.span.end_byte <= span.end_byte;
                if !in_declaration {
                    reassigned = true;
                    break;
                }
                match earliest {
                    Some((start, _)) if reference.span.start_byte > start => {}
                    Some((start, _)) if reference.span.start_byte == start => tied = true,
                    _ => {
                        earliest = Some((reference.span.start_byte, reference));
                        tied = false;
                    }
                }
            }
            if reassigned || tied {
                continue;
            }
            let Some((_, initializer)) = earliest else {
                continue;
            };
            let key = (ext.file_path.clone(), name.to_string());
            if self.unique_indexed_type(family, &initializer.name) {
                self.module_value_types
                    .insert(key, initializer.name.clone());
            } else if initializer.kind == ReferenceKind::Call {
                // A factory. Its declaration may sit in a file not yet indexed,
                // so the shape is kept and resolved when it is read.
                if let Some(shape) = initializer.initializer_shape(&ext.calls) {
                    self.module_value_factories.insert(key, (family, shape));
                }
            }
        }
    }

    /// The class a module-scope value `name` declared in `file` is an instance
    /// of, following a re-export chain to the file that declares it, and — for
    /// Go — to the sibling file of the same package that declares it.
    pub(super) fn module_value_type(&self, file: &str, name: &str) -> Option<String> {
        if let Some(found) = self.module_value_type_in(file, name) {
            return Some(found);
        }
        if let Some(terminal) = self.reexport_chains.get(&format!("{file}::{name}")) {
            let (declaring_file, declared) = terminal.rsplit_once("::")?;
            return self.module_value_type_in(declaring_file, declared);
        }
        self.go_package_value_type(file, name)
    }

    /// The type one file's own declaration of `name` gives it: a constructor
    /// initializer recorded at index time, or a factory resolved now.
    pub(super) fn module_value_type_in(&self, file: &str, name: &str) -> Option<String> {
        let key = (file.to_string(), name.to_string());
        if let Some(found) = self.module_value_types.get(&key) {
            return Some(found.clone());
        }
        let (family, shape) = self.module_value_factories.get(&key)?;
        self.factory_return_type(*family, file, None, shape)
    }

    /// A Go package-level value declared in another file of `file`'s package.
    ///
    /// A Go package is one namespace spread over its files, so
    /// `defaultRegistry.Add()` in `register.go` names the `var` in
    /// `registry.go` with no import between them. Only an answer every
    /// declaring file agrees on is given: two build-constrained files may each
    /// declare the name, and they need not agree.
    pub(super) fn go_package_value_type(&self, file: &str, name: &str) -> Option<String> {
        if !file.ends_with(".go") || !Self::is_plain_ident(name) {
            return None;
        }
        let package = self.go_package_by_file.get(file)?;
        let mut found: Option<String> = None;
        for sibling in self.go_files_in_dir(&Self::parent_dir(file)) {
            if sibling == file || self.go_package_by_file.get(&sibling) != Some(package) {
                continue;
            }
            let key = (sibling.clone(), name.to_string());
            if !self.module_value_types.contains_key(&key)
                && !self.module_value_factories.contains_key(&key)
            {
                continue;
            }
            let typed = self.module_value_type_in(&sibling, name)?;
            match &found {
                Some(existing) if existing != &typed => return None,
                _ => found = Some(typed),
            }
        }
        found
    }

    /// The language family a file was indexed under, read from its own `File`
    /// symbol so it cannot disagree with the family its symbols carry.
    pub(super) fn family_of_file(&self, file: &str) -> Option<LangFamily> {
        self.symbol_index
            .get(file)?
            .iter()
            .find_map(|(path, kind, family, _)| {
                (path == file && *kind == SymbolKind::File).then_some(*family)
            })
    }

    /// The nominal type a factory call returns, from the callee's written
    /// return type: `NewRegistry` → `Registry` for `func NewRegistry()
    /// *Registry`, `svc.make` → `Svc`, `Svc::open` → `Svc`.
    ///
    /// The callee is found the way the call ladder would find it, and only by
    /// rungs that name one declaration: a file-level function of this file (or
    /// of its Go package), a named import, a member of an imported module
    /// handle, or an associated function of a type. A bare callee the scope
    /// binds itself — a parameter or a local closure named `make` — is not the
    /// module's `make`, so it abstains. The answer must be a single indexed
    /// class or struct; a generic, a tuple, `Promise<T>` or an optional names
    /// no type a method can be dispatched on, and `admissible_nominal_type`
    /// refuses each.
    pub(super) fn factory_return_type(
        &self,
        family: LangFamily,
        file: &str,
        scope: Option<&str>,
        shape: &str,
    ) -> Option<String> {
        let (callee_file, callee) = self.factory_callee(family, file, scope, shape)?;
        self.factory_written_type(family, file, &callee_file, &callee)
    }

    /// The one callable an initializer shape names, as `(file, qualified
    /// name)`: the half of [`Self::factory_return_type`] that finds the callee.
    pub(super) fn factory_callee(
        &self,
        family: LangFamily,
        file: &str,
        scope: Option<&str>,
        shape: &str,
    ) -> Option<(String, String)> {
        let shape = shape.trim();
        if shape.ends_with("{..}") || shape.ends_with("::new") {
            // Constructor shapes have their own rung, which needs no callee.
            return None;
        }
        let operator = (family == LangFamily::Swift
            && !shape.is_empty()
            && !shape.chars().any(|c| c.is_alphanumeric() || c == '_'))
        .then(|| {
            // `a <+> b`: the operator's own declaration says what it returns.
            let mut declared =
                self.symbol_index
                    .get(shape)?
                    .iter()
                    .filter(|(_, kind, candidate, _)| {
                        *kind == SymbolKind::Method && family.admits(*candidate)
                    });
            let (file, _, _, identity) = declared.next()?;
            declared
                .next()
                .is_none()
                .then(|| (file.clone(), identity.to_string()))
        });
        let (callee_file, callee) = if let Some(found) = operator {
            found?
        } else if let Some((owner, function)) = shape.rsplit_once("::") {
            let owner = owner.rsplit("::").next()?;
            self.unique_type_method(family, owner, function)?
        } else if let Some((receiver, function)) = shape.rsplit_once('.') {
            if !Self::is_plain_ident(receiver)
                || !Self::is_plain_ident(function)
                || scope.is_some_and(|scope| self.scope_shadows(family, file, scope, receiver))
            {
                return None;
            }
            let module = self
                .import_binding_at(family, file, scope, receiver)
                .filter(|_| !self.is_value_import(file, receiver));
            match module {
                Some((module_file, _)) => self.lookup_in_package(module_file, function)?,
                // `Svc.create()`: a static factory on a type this corpus
                // declares once.
                None if self.unique_indexed_type(family, receiver) => {
                    self.unique_type_method(family, receiver, function)?
                }
                None => return None,
            }
        } else {
            if !Self::is_plain_ident(shape)
                || scope.is_some_and(|scope| self.scope_shadows(family, file, scope, shape))
                || scope.is_some_and(|scope| {
                    self.enclosing_type_declares_member(family, file, scope, shape)
                })
            {
                return None;
            }
            match self.lookup_in_package(file, shape) {
                Some(found) => found,
                None => {
                    if family == LangFamily::Python
                        && !self.python_bare_name_has_one_provider(file, shape)
                    {
                        return None;
                    }
                    let (target, declared) = self.import_binding_at(family, file, scope, shape)?;
                    self.lookup_in_package(target, declared)?
                }
            }
        };
        // A Python callable a decorator may wrap returns whatever the decorator
        // makes of it, not what its own annotation says.
        if family == LangFamily::Python
            && self
                .python_decorated
                .contains(&(callee_file.clone(), callee.clone()))
        {
            return None;
        }
        Some((callee_file, callee))
    }

    /// Whether a type enclosing `scope` declares a member called `name`, in a
    /// language whose bare call inside a method is first the enclosing type's
    /// own member (Kotlin, Swift, Java and the other brace languages).
    ///
    /// A bare `make()` written in `class C` is `this.make()` when `C` declares
    /// one, and only otherwise a top-level `make`; when both exist the callee is
    /// not the top-level one the file-level lookup finds, so the factory rungs
    /// abstain rather than type the local by the wrong function.
    pub(super) fn enclosing_type_declares_member(
        &self,
        family: LangFamily,
        file: &str,
        scope: &str,
        name: &str,
    ) -> bool {
        if !matches!(
            family,
            LangFamily::Kotlin | LangFamily::Swift | LangFamily::CStyle
        ) {
            return false;
        }
        let declares = |type_identity: &str| {
            let bare = type_identity.rsplit("::").next().unwrap_or(type_identity);
            self.type_methods
                .contains_key(&(family, bare.to_string(), name.to_string()))
        };
        // A call attributed to the type itself (a property initializer) sees its
        // own members; a call in a method sees its type's, then the outer ones.
        let mut current = scope.to_string();
        for _ in 0..8 {
            if current != file && declares(&current) {
                return true;
            }
            let Some(parent) = self
                .symbol_parents
                .get(&(file.to_string(), current.clone()))
                .filter(|parent| parent.as_str() != file && **parent != current)
            else {
                return false;
            };
            if declares(parent) {
                return true;
            }
            current = parent.clone();
        }
        false
    }

    /// The nominal type the callable `(callee_file, callee)` is written to
    /// return, read at a call site in `file`.
    pub(super) fn factory_written_type(
        &self,
        family: LangFamily,
        file: &str,
        callee_file: &str,
        callee: &str,
    ) -> Option<String> {
        let written = self
            .return_types
            .get(&(callee_file.to_string(), callee.to_string()))?;
        let mut nominal = Self::admissible_nominal_type(written)?;
        if nominal == "Self" {
            nominal = self.declaring_type_of(callee_file, callee)?.to_string();
        }
        // The annotation is a name in the callee's file, which the caller may
        // not share: read it there (an alias is the class it aliases).
        if family == LangFamily::Python && callee_file != file {
            nominal = self.python_annotation_class(callee_file, &nominal)?;
        }
        // Kotlin reads a bare name through its imports and package, so the one
        // corpus declaration must also be the one both files' spellings reach
        // (not a nested class, not shadowed by a single-name import).
        if family == LangFamily::Kotlin
            && !self.kotlin_constructor_is_one_type(file, callee_file, &nominal)
        {
            return None;
        }
        (self.unique_indexed_type(family, &nominal)
            || self.names_one_type_in_both(file, callee_file, &nominal))
        .then_some(nominal)
    }

    /// Fill [`Self::kotlin_initializer_calls`] for one Kotlin file: for each
    /// local, every callee named in its initializer (`if (f) a() else b()`
    /// names both; `a(b())` names both).
    pub(super) fn index_kotlin_initializer_calls(&mut self, ext: &Extraction) {
        for reference in ext.references.iter().filter(|reference| {
            matches!(
                reference.kind,
                ReferenceKind::Call | ReferenceKind::Constructor
            )
        }) {
            let Some(bound) = reference.assigned_to.as_deref() else {
                continue;
            };
            let scope = reference
                .enclosing_symbol
                .clone()
                .unwrap_or_else(|| ext.file_path.clone());
            let callees = self
                .kotlin_initializer_calls
                .entry((ext.file_path.clone(), scope, bound.to_string()))
                .or_default();
            if !callees.contains(&reference.name) {
                callees.push(reference.name.clone());
            }
        }
    }

    /// The type a Kotlin local gets from an initializer whose callee is written
    /// to return a *generic* type: `val files = names()` with `fun names():
    /// List<String>` is a `List`.
    ///
    /// The constructor is a written nominal type, but dropping its arguments
    /// must not let a stdlib-spelled name reach a same-named class of the repo,
    /// so it binds only when:
    ///
    /// * the written type is one unqualified, un-nullable `Name<...>` (see
    ///   [`Self::kotlin_type_constructor`]) — a qualified or aliased spelling
    ///   names whatever its qualifier says;
    /// * the corpus declares no type of that name and the callee and the call
    ///   are in one file (a stdlib or external type: one import list governs
    ///   both spellings), or the corpus declares exactly one and both files
    ///   reach that declaration by an explicit import of it or by sharing its
    ///   package — never for a name Kotlin imports by default unless the
    ///   declaration is in the same file or imported explicitly, and never when
    ///   the file imports that name from elsewhere;
    /// * every callee the initializer names agrees on the type (an `if`/`when`
    ///   with two different branch types, or a call nested in an argument whose
    ///   type is not known, abstains).
    pub(super) fn kotlin_generic_factory_type(
        &self,
        file: &str,
        binding: &LocalBinding,
    ) -> Option<String> {
        let shape = binding.initializer.as_deref()?;
        let (callee_file, callee) =
            self.factory_callee(LangFamily::Kotlin, file, binding.scope.as_deref(), shape)?;
        let written = self.return_types.get(&(callee_file.clone(), callee))?;
        let constructor = Self::kotlin_type_constructor(written)?;
        if !self.kotlin_constructor_is_one_type(file, &callee_file, &constructor) {
            return None;
        }
        let scope = binding.scope.clone().unwrap_or_else(|| file.to_string());
        let callees =
            self.kotlin_initializer_calls
                .get(&(file.to_string(), scope, binding.name.clone()))?;
        callees
            .iter()
            .all(|other| {
                other == shape
                    || self
                        .kotlin_callee_constructor(file, binding.scope.as_deref(), other)
                        .is_some_and(|found| found == constructor)
            })
            .then_some(constructor)
    }

    /// The type constructor a named Kotlin callee yields: its written generic
    /// return type, or — for a stdlib collection factory the corpus does not
    /// redeclare — the collection it builds.
    fn kotlin_callee_constructor(
        &self,
        file: &str,
        scope: Option<&str>,
        callee: &str,
    ) -> Option<String> {
        if let Some((callee_file, identity)) =
            self.factory_callee(LangFamily::Kotlin, file, scope, callee)
        {
            let written = self.return_types.get(&(callee_file, identity))?;
            return Self::kotlin_type_constructor(written);
        }
        if self.family_declares(LangFamily::Kotlin, callee) {
            return None;
        }
        let built = match callee {
            "emptyList" | "listOf" => "List",
            "emptySet" | "setOf" => "Set",
            "emptyMap" | "mapOf" => "Map",
            _ => return None,
        };
        Some(built.to_string())
    }

    /// Whether the type constructor `constructor`, written in `callee_file` and
    /// read at a call site in `file`, is one type. See
    /// [`Self::kotlin_generic_factory_type`] for the rule.
    pub(super) fn kotlin_constructor_is_one_type(
        &self,
        file: &str,
        callee_file: &str,
        constructor: &str,
    ) -> bool {
        let declared: Vec<&(String, SymbolKind, LangFamily, Arc<str>)> = self
            .symbol_index
            .get(constructor)
            .into_iter()
            .flatten()
            .filter(|(_, kind, family, _)| {
                // A function or method sharing the name (a `fun Repo()` factory
                // beside `class Repo`) is not a type.
                LangFamily::Kotlin.admits(*family)
                    && !matches!(kind, SymbolKind::Function | SymbolKind::Method)
            })
            .collect();
        let [(declaring_file, kind, _, identity)] = declared.as_slice() else {
            // None declared: a stdlib or external type. Two or more: no one
            // declaration for either spelling to mean.
            return declared.is_empty() && file == callee_file;
        };
        // A nested class (`Outer.Repo`) is not what a bare `Repo` names outside
        // `Outer`.
        identity
            .strip_prefix(declaring_file.as_str())
            .and_then(|rest| rest.strip_prefix("::"))
            == Some(constructor)
            && matches!(
                kind,
                SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface | SymbolKind::Enum
            )
            && [callee_file, file]
                .iter()
                .all(|site| self.kotlin_site_names_declaration(site, constructor, declaring_file))
    }

    /// Whether `site` reads the bare type name `name` as the declaration in
    /// `declaring_file`: it declares it, imports exactly it, or — for a name
    /// Kotlin does not import by default — shares its package. A file that
    /// imports the name from anywhere else binds it there.
    pub(super) fn kotlin_site_names_declaration(
        &self,
        site: &str,
        name: &str,
        declaring_file: &str,
    ) -> bool {
        // A single-name import of the name from outside the index binds it
        // there, and beats a star import in either order.
        if self
            .external_imports
            .get(site)
            .is_some_and(|imports| imports.contains_key(name))
            || self
                .unindexed_local_imports
                .get(site)
                .is_some_and(|imports| imports.contains_key(name))
        {
            return false;
        }
        if let Some((target, _)) = self
            .import_bindings
            .get(site)
            .and_then(|bindings| bindings.get(name))
        {
            return target == declaring_file;
        }
        if site == declaring_file {
            return true;
        }
        if KOTLIN_DEFAULT_IMPORTS.contains(&name) {
            return false;
        }
        matches!(
            (
                self.jvm_package_by_file.get(site),
                self.jvm_package_by_file.get(declaring_file),
            ),
            (Some(here), Some(there)) if here == there
        )
    }

    /// Whether the callable `scope` binds `name` itself, so a read of it there is
    /// not the module's or an import's. Python asks its scope walk, which also
    /// sees a nested `def`/`class` and the enclosing functions; a function-scope
    /// import is not a shadow here, because the import lookup reads it.
    pub(super) fn scope_shadows(
        &self,
        family: LangFamily,
        file: &str,
        scope: &str,
        name: &str,
    ) -> bool {
        if family == LangFamily::Python {
            return scope != file
                && matches!(
                    self.python_scope_lookup(file, scope, name, 0),
                    Some(super::pytypes::ScopeHit::Local)
                );
        }
        self.scope_declares_local(file, scope, name)
    }

    /// Whether `nominal`, read where the factory wrote it and where the call is
    /// written, is the same one type declaration.
    ///
    /// A type name is what a receiver map holds, and the method rung reads it
    /// in the *caller's* scope — which is how `client := &Client{}` dispatches
    /// in a corpus with a `Client` in every package. A factory's return type is
    /// written in the *callee's* scope, so a name the corpus declares twice is
    /// only safe to hand on when both scopes bind it to one declaration: the
    /// same file, or the same Go package. Anywhere else the caller's `Client`
    /// may be another type, and the corpus-wide uniqueness test decides.
    pub(super) fn names_one_type_in_both(
        &self,
        file: &str,
        callee_file: &str,
        nominal: &str,
    ) -> bool {
        let Some(here) = self.lookup_in_package(file, nominal) else {
            return false;
        };
        if self.lookup_in_package(callee_file, nominal).as_ref() != Some(&here) {
            return false;
        }
        self.symbol_index.get(nominal).is_some_and(|candidates| {
            candidates.iter().any(|(path, kind, _, identity)| {
                *path == here.0
                    && **identity == *here.1
                    && matches!(kind, SymbolKind::Class | SymbolKind::Struct)
            })
        })
    }

    /// The one method `type_name` declares called `method`, as `(file,
    /// qualified name)`, or `None` when it declares none or the type name is
    /// declared twice.
    pub(super) fn unique_type_method(
        &self,
        family: LangFamily,
        type_name: &str,
        method: &str,
    ) -> Option<(String, String)> {
        let hits = self
            .type_methods
            .get(&(family, type_name.to_string(), method.to_string()))?;
        let [only] = hits.as_slice() else {
            return None;
        };
        Some(only.clone())
    }

    /// The type of a receiver this file reaches through an import: a named
    /// import of a module-scope instance (`import { svc }`, `from m import
    /// svc`), or one member of an imported module handle (`ns.svc`,
    /// `service_module.service`).
    pub(super) fn imported_receiver_type(&self, file: &str, receiver: &str) -> Option<String> {
        if Self::is_plain_ident(receiver) {
            let (declaring_file, declared) = self
                .value_imports
                .get(&(file.to_string(), receiver.to_string()))?;
            return self.module_value_type(declaring_file, declared);
        }
        let (root, member) = Self::one_hop_field(receiver)?;
        if self.is_value_import(file, root) {
            return None;
        }
        let (module_file, _) = self.import_bindings.get(file)?.get(root)?;
        self.module_value_type(module_file, member)
    }

    /// Whether `member` may be read as a module-level declaration when it is
    /// reached through `receiver`, a name `file` imports.
    ///
    /// A module handle: always — that is what a module member is. An imported
    /// type (`Page.SystematicReview`, `SvcClass.build()`): always, the static
    /// shape. A named import of any other value: only when the value's own
    /// declaration spells the member, which is the object-literal namespace
    /// (`api = { get }`), and never when it is a typed instance — rung 1 owns
    /// that answer, and a method its class lacks is not the module's function.
    pub(super) fn module_member_admitted(&self, file: &str, receiver: &str, member: &str) -> bool {
        let Some((declaring_file, declared)) = self
            .value_imports
            .get(&(file.to_string(), receiver.to_string()))
        else {
            return true;
        };
        if self.imports_a_type(file, receiver) {
            return true;
        }
        if self.module_value_type(declaring_file, declared).is_some() {
            return false;
        }
        self.module_value_members
            .get(&(declaring_file.clone(), declared.clone()))
            .is_some_and(|members| members.contains(member))
    }

    /// Whether `local` in `file` is a named import of a declared value rather
    /// than a module handle. See [`Self::value_imports`].
    pub(super) fn is_value_import(&self, file: &str, local: &str) -> bool {
        self.value_imports
            .contains_key(&(file.to_string(), local.to_string()))
    }

    /// Whether the value `local` imports into `file` is itself a type
    /// declaration. `SvcClass.build()` on an imported class is a static call,
    /// and the literal-type rung below the import rungs answers it; only a
    /// non-type value stops the ladder when its type is unknown.
    pub(super) fn imports_a_type(&self, file: &str, local: &str) -> bool {
        let Some((declaring_file, declared)) = self
            .value_imports
            .get(&(file.to_string(), local.to_string()))
        else {
            return false;
        };
        self.symbol_index.get(declared).is_some_and(|candidates| {
            candidates.iter().any(|(path, kind, _, _)| {
                path == declaring_file
                    && matches!(
                        kind,
                        SymbolKind::Class
                            | SymbolKind::Struct
                            | SymbolKind::Enum
                            | SymbolKind::Interface
                            | SymbolKind::Trait
                    )
            })
        })
    }

    /// Whether the declaration a named import binds owns a member `member` in
    /// its own file — a factory whose returned object literal declares it
    /// (`registryAdapter(run).tagsForDigest()`), recorded by the extractor as
    /// `registryAdapter.tagsForDigest`. The literal-type rung answers that
    /// shape from the import's own declaring file, so the import-scoped stop
    /// must leave it to that rung rather than pre-empt it.
    pub(super) fn imported_declaration_owns(
        &self,
        family: LangFamily,
        file: &str,
        local: &str,
        member: &str,
    ) -> bool {
        let Some((declaring_file, declared)) = self
            .value_imports
            .get(&(file.to_string(), local.to_string()))
        else {
            return false;
        };
        self.type_methods
            .get(&(family, declared.clone(), member.to_string()))
            .is_some_and(|hits| hits.iter().any(|(path, _)| path == declaring_file))
    }

    /// Record `key -> type_name`, poisoning the key if a second type claims it.
    ///
    /// Last-write-wins here is what made SC9 a *confidently wrong* answer rather
    /// than a missing one: the loser of the race still got resolved, at
    /// DETERMINISTIC confidence, to the wrong type.
    pub(super) fn bind_receiver(
        map: &mut BTreeMap<String, String>,
        poisoned: &mut BTreeSet<String>,
        key: String,
        type_name: &str,
    ) {
        if poisoned.contains(&key) {
            return;
        }
        match map.get(&key) {
            Some(existing) if existing != type_name => {
                map.remove(&key);
                poisoned.insert(key);
            }
            _ => {
                map.insert(key, type_name.to_string());
            }
        }
    }
}
