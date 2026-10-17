//! Python class identity at a call site: which class a written name denotes,
//! and which declaration a method call on it reaches.
//!
//! `type_methods` and `supertypes` are flat by bare class name, so two files
//! that each declare an `Engine` collapse into one key and a call on either
//! looks ambiguous — or, worse, a lone hit in the *other* file reads as the
//! answer. Python says which class a name is: the file's own `class` statement
//! wins, and otherwise the `import` that bound the name. This module asks that
//! question first and only then reads the flat tables, restricted to the file
//! the name was proven to live in.

use super::*;

/// What a Python call on a class resolves to: the declaring file, the method's
/// identity, and the class that declares it (which is a base class when the
/// method is inherited).
pub(super) struct PythonMethod {
    pub(super) file: String,
    pub(super) symbol: String,
    pub(super) declaring_type: String,
}

impl Resolver {
    /// The class `written` denotes in `file`, as `(declaring file, class
    /// identity)`: declared at file level here, or bound by an `import` to a
    /// class declared elsewhere. `None` when the name is neither, or is
    /// declared twice in the file.
    pub(super) fn python_class_named(&self, file: &str, written: &str) -> Option<(String, String)> {
        let is_class = |path: &str, identity: &str| {
            self.symbol_index
                .get(identity.rsplit("::").next().unwrap_or(identity))
                .into_iter()
                .flatten()
                .any(|(candidate, kind, family, candidate_identity)| {
                    candidate == path
                        && **candidate_identity == *identity
                        && *kind == SymbolKind::Class
                        && *family == LangFamily::Python
                })
        };
        let local: Vec<&Arc<str>> = self
            .symbol_index
            .get(written)
            .into_iter()
            .flatten()
            .filter(|(path, kind, family, identity)| {
                path == file
                    && *kind == SymbolKind::Class
                    && *family == LangFamily::Python
                    && self.declared_at_file_level(path, identity)
            })
            .map(|(_, _, _, identity)| identity)
            .collect();
        match local.as_slice() {
            [only] => return Some((file.to_string(), only.to_string())),
            [] => {}
            _ => return None,
        }
        let (target_file, target_symbol) = self.import_bindings.get(file)?.get(written)?;
        let (class_file, identity) = self.lookup_in_package(target_file, target_symbol)?;
        is_class(&class_file, &identity).then_some((class_file, identity))
    }

    /// The bare type name `type_methods` and `supertypes` key a class by.
    fn python_type_key(identity: &str) -> &str {
        identity.rsplit("::").next().unwrap_or(identity)
    }

    /// The method `method` on the class `written` names in `file`: the class's
    /// own declaration, else the nearest declared base's. `None` — with no
    /// fallback to a same-named class elsewhere — when the class is known and
    /// the method is not found or not unique; `Err(())` when `written` is no
    /// class this file can name, so the caller may read the flat tables.
    pub(super) fn python_method_on(
        &self,
        file: &str,
        written: &str,
        method: &str,
    ) -> Result<Option<PythonMethod>, ()> {
        let (class_file, identity) = self.python_class_named(file, written).ok_or(())?;
        let type_key = Self::python_type_key(&identity);
        let own = self.python_own_methods(&class_file, type_key, method);
        match own.as_slice() {
            [(path, symbol)] => {
                return Ok(Some(PythonMethod {
                    file: path.clone(),
                    symbol: symbol.clone(),
                    declaring_type: type_key.to_string(),
                }));
            }
            [] => {}
            _ => return Ok(None),
        }
        Ok(self.python_inherited_from(&class_file, type_key, method))
    }

    /// The declarations of `method` that the class `type_key`, declared in
    /// `class_file`, writes on itself.
    fn python_own_methods(
        &self,
        class_file: &str,
        type_key: &str,
        method: &str,
    ) -> Vec<&(String, String)> {
        self.type_methods
            .get(&(LangFamily::Python, type_key.to_string(), method.to_string()))
            .into_iter()
            .flatten()
            .filter(|(path, _)| path == class_file)
            .collect()
    }

    /// `method` as the bases of the class `type_key` (declared in `class_file`)
    /// provide it: the file-scoped walk first, the flat table only when that
    /// walk finds nothing.
    ///
    /// The flat read refuses any class name two files declare, so a collision
    /// in the child's or a base's name used to lose the edge. The scoped walk
    /// asks Python's own question instead — which class does this file's
    /// `class` statement or `import` bind the base name to. A scoped answer is
    /// always a binding of a name this file resolved; the flat fallback keeps
    /// every answer the resolver gave before and still abstains on a collision.
    pub(super) fn python_inherited_from(
        &self,
        class_file: &str,
        type_key: &str,
        method: &str,
    ) -> Option<PythonMethod> {
        self.python_scoped_inherited_method(class_file, type_key, method)
            .or_else(|| self.python_inherited_method(type_key, method))
    }

    /// The one method `method` that the declared bases of `type_key` (declared
    /// in `class_file`) provide, walking each base through the file that names
    /// it. Breadth-first, like [`Self::supertype_method`], so a method is found
    /// at the level it is first declared, and abstaining on the same
    /// ambiguities: two bases of one level that both declare it, or a class
    /// that declares it twice. This resolver has no MRO, so a diamond answers
    /// only when exactly one base on the nearest level provides the method.
    ///
    /// `None` also when a base is declared in the corpus but cannot be bound to
    /// one class (declared twice in its file, reached through a qualified name
    /// or star import), because binding it by its flat name is exactly what
    /// this walk exists to avoid. A base the corpus does not declare at all
    /// (`object`, `ABC`) provides nothing indexed and is skipped.
    fn python_scoped_inherited_method(
        &self,
        class_file: &str,
        type_key: &str,
        method: &str,
    ) -> Option<PythonMethod> {
        let mut frontier: Vec<(String, String)> =
            vec![(class_file.to_string(), type_key.to_string())];
        let mut visited: BTreeSet<(String, String)> = BTreeSet::new();
        for _ in 0..Self::HERITAGE_WALK_MAX_DEPTH {
            let mut found: BTreeSet<(String, String, String)> = BTreeSet::new();
            let mut next: Vec<(String, String)> = Vec::new();
            for (file, class) in &frontier {
                if !visited.insert((file.clone(), class.clone())) {
                    continue;
                }
                let Some(bases) = self.python_bases.get(&(file.clone(), class.clone())) else {
                    continue;
                };
                for base in bases {
                    let Some((base_file, identity)) = self.python_class_named(file, base) else {
                        if self.declares_type(LangFamily::Python, base) {
                            return None;
                        }
                        continue;
                    };
                    let base_key = Self::python_type_key(&identity);
                    match self
                        .python_own_methods(&base_file, base_key, method)
                        .as_slice()
                    {
                        [] => {}
                        [(path, symbol)] => {
                            found.insert((path.clone(), symbol.clone(), base_key.to_string()));
                        }
                        _ => return None,
                    }
                    next.push((base_file, base_key.to_string()));
                }
            }
            match found.len() {
                1 => {
                    let (file, symbol, declaring_type) = found.into_iter().next()?;
                    return Some(PythonMethod {
                        file,
                        symbol,
                        declaring_type,
                    });
                }
                0 => {}
                _ => return None,
            }
            if next.is_empty() {
                return None;
            }
            frontier = next;
        }
        None
    }

    /// `method` as the declared bases of `type_key` provide it. Needs the class
    /// name to be unique in the corpus, because `supertypes` is keyed by it.
    pub(super) fn python_inherited_method(
        &self,
        type_key: &str,
        method: &str,
    ) -> Option<PythonMethod> {
        if !self.type_name_is_identifiable(LangFamily::Python, type_key) {
            return None;
        }
        let bases: Vec<String> = self
            .supertypes
            .get(&(LangFamily::Python, type_key.to_string()))?
            .iter()
            .cloned()
            .collect();
        let (file, symbol, declaring_type) =
            self.supertype_method(LangFamily::Python, bases, method)?;
        Some(PythonMethod {
            file,
            symbol,
            declaring_type,
        })
    }

    /// The method a `super().method()` call reaches from a call attributed to
    /// `caller`: the enclosing class's declared bases, never the enclosing
    /// class itself.
    pub(super) fn python_super_method(
        &self,
        file: &str,
        caller: &str,
        method: &str,
    ) -> Option<PythonMethod> {
        let enclosing = self.self_type_at(file, LangFamily::Python, caller)?;
        self.python_inherited_from(file, &enclosing, method)
    }

    /// The class a `cls(...)` call inside a method of it constructs.
    pub(super) fn python_cls_target(&self, file: &str, caller: &str) -> Option<(String, String)> {
        let enclosing = self.self_type_at(file, LangFamily::Python, caller)?;
        self.python_class_named(file, &enclosing)
    }
}
