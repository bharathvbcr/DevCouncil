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

/// A class in a Python MRO: one this corpus declares, or a base that is not
/// in it (external, builtin, or an import that names no class here) and so may
/// define any method.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum MroNode {
    Class(String, String),
    Opaque(String),
}

/// Why the file-scoped walk gave no linearization.
pub(super) enum MroStop {
    /// A base this file cannot bind or prove external: ask the flat table.
    NotScoped,
    /// A cycle, an inconsistent hierarchy or a bound hit: answer nothing.
    Abstain,
}

pub(super) enum MroAnswer {
    Found(PythonMethod),
    Abstain,
    NotScoped,
}

impl Resolver {
    /// Deepest chain of bases the C3 walk follows.
    const PYTHON_MRO_MAX_DEPTH: usize = 64;
    /// Most classes the C3 walk may linearize for one question.
    const PYTHON_MRO_MAX_NODES: usize = 512;

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
    /// provide it: the file-scoped C3 walk first, the flat table only when that
    /// walk cannot speak for the class.
    ///
    /// The flat read refuses any class name two files declare, so a collision
    /// in the child's or a base's name used to lose the edge. The scoped walk
    /// asks Python's own question instead — which class does this file's
    /// `class` statement or `import` bind the base name to — and orders the
    /// answer the way Python does. It speaks, in either direction, for every
    /// base it can bind or prove external; it hands the question to the flat
    /// table only for a base it can neither bind nor prove external (a star
    /// import, a name declared twice in its file), where that table still
    /// abstains on a collision.
    pub(super) fn python_inherited_from(
        &self,
        class_file: &str,
        type_key: &str,
        method: &str,
    ) -> Option<PythonMethod> {
        match self.python_mro_method(class_file, type_key, method) {
            MroAnswer::Found(found) => Some(found),
            MroAnswer::Abstain => None,
            MroAnswer::NotScoped => self.python_inherited_method(type_key, method),
        }
    }

    /// Whether `identity`, declared in `path`, is a Python class.
    fn python_is_class(&self, path: &str, identity: &str) -> bool {
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
    }

    /// Whether `file` binds `name` with an `import` of any kind: one that
    /// resolved to an indexed module, one whose module is outside the corpus,
    /// or a relative one that names no indexed file. A star import binds no
    /// particular name and does not count.
    fn python_name_is_imported(&self, file: &str, name: &str) -> bool {
        self.import_bindings
            .get(file)
            .is_some_and(|bound| bound.contains_key(name))
            || self
                .external_imports
                .get(file)
                .is_some_and(|bound| bound.contains_key(name))
            || self
                .unindexed_local_imports
                .get(file)
                .is_some_and(|bound| bound.contains_key(name))
    }

    /// The MRO node a base written in `file` as `written` denotes.
    ///
    /// A class this file names (its own `class`, or the import that bound it)
    /// is [`MroNode::Class`]. A base that is provably not in the corpus — bound
    /// by an import to a module outside it, reached through a dotted name whose
    /// root is such an import, or a name nothing declares and nothing imports
    /// (`dict`, `Exception`) — is [`MroNode::Opaque`]: it may define any
    /// method, so it blocks a lookup that would pass through it. A name the
    /// corpus declares that this file cannot bind to one declaration is
    /// [`MroStop::NotScoped`].
    fn python_mro_node(&self, file: &str, written: &str) -> Result<MroNode, MroStop> {
        if let Some((root, member)) = written.split_once('.') {
            let Some((module_file, _)) = self
                .import_bindings
                .get(file)
                .and_then(|bound| bound.get(root))
                .filter(|_| !self.is_value_import(file, root))
            else {
                return if self.python_name_is_imported(file, root) {
                    Ok(MroNode::Opaque(written.to_string()))
                } else if self.declares_type(
                    LangFamily::Python,
                    member.rsplit('.').next().unwrap_or(member),
                ) {
                    Err(MroStop::NotScoped)
                } else {
                    Ok(MroNode::Opaque(written.to_string()))
                };
            };
            // `import pkg.mod as m; class C(m.Base)`: a member of an indexed
            // module is that module's class, or nothing this walk can name.
            return Ok(match member.contains('.') {
                false => self
                    .lookup_in_package(module_file, member)
                    .filter(|(path, identity)| self.python_is_class(path, identity))
                    .map_or_else(
                        || MroNode::Opaque(written.to_string()),
                        |(path, identity)| MroNode::Class(path, identity),
                    ),
                true => MroNode::Opaque(written.to_string()),
            });
        }
        if let Some((path, identity)) = self.python_class_named(file, written) {
            return Ok(MroNode::Class(path, identity));
        }
        if self.python_name_is_imported(file, written) {
            return Ok(MroNode::Opaque(written.to_string()));
        }
        if self.declares_type(LangFamily::Python, written) {
            return Err(MroStop::NotScoped);
        }
        Ok(MroNode::Opaque(written.to_string()))
    }

    /// The C3 linearization of `node` (the node first), memoised.
    ///
    /// Bounded by [`Self::PYTHON_MRO_MAX_DEPTH`] and by the number of nodes
    /// the walk may create; an inheritance cycle, an inconsistent hierarchy
    /// (where C3 has no answer — Python raises `TypeError` there) and a
    /// bound hit are all [`MroStop::Abstain`].
    fn python_linearize(
        &self,
        node: &MroNode,
        depth: usize,
        memo: &mut BTreeMap<MroNode, Vec<MroNode>>,
        in_progress: &mut BTreeSet<MroNode>,
    ) -> Result<Vec<MroNode>, MroStop> {
        if let Some(done) = memo.get(node) {
            return Ok(done.clone());
        }
        let MroNode::Class(file, identity) = node else {
            return Ok(vec![node.clone()]);
        };
        if depth > Self::PYTHON_MRO_MAX_DEPTH
            || memo.len() > Self::PYTHON_MRO_MAX_NODES
            || !in_progress.insert(node.clone())
        {
            return Err(MroStop::Abstain);
        }
        let key = Self::python_type_key(identity).to_string();
        let written: &[String] = match self.python_bases.get(&(file.clone(), key)) {
            Some(bases) => bases,
            // The root of the walk has no recorded heritage: nothing here says
            // whether that is a class with no bases or a key this table never
            // learned, so the flat table is asked instead of answering "none".
            None if depth == 0 => return Err(MroStop::NotScoped),
            None => &[],
        };
        let mut bases: Vec<MroNode> = Vec::new();
        for base in written {
            bases.push(self.python_mro_node(file, base)?);
        }
        let mut sequences: Vec<Vec<MroNode>> = Vec::new();
        for base in &bases {
            sequences.push(self.python_linearize(base, depth + 1, memo, in_progress)?);
        }
        sequences.push(bases);
        let mut linear = vec![node.clone()];
        loop {
            sequences.retain(|sequence| !sequence.is_empty());
            if sequences.is_empty() {
                break;
            }
            // The first head that appears in no sequence's tail.
            let head = sequences
                .iter()
                .map(|sequence| &sequence[0])
                .find(|head| sequences.iter().all(|other| !other[1..].contains(head)))
                .cloned()
                .ok_or(MroStop::Abstain)?;
            for sequence in &mut sequences {
                if sequence[0] == head {
                    sequence.remove(0);
                }
            }
            linear.push(head);
        }
        in_progress.remove(node);
        memo.insert(node.clone(), linear.clone());
        Ok(linear)
    }

    /// The one method `method` that the first class in the MRO of `type_key`
    /// (declared in `class_file`) declares, after the class itself.
    ///
    /// An [`MroNode::Opaque`] class ahead of that class abstains: the external
    /// or builtin base comes first in Python's order and might define the
    /// method itself. A class that declares the name twice abstains too.
    fn python_mro_method(&self, class_file: &str, type_key: &str, method: &str) -> MroAnswer {
        let root = MroNode::Class(class_file.to_string(), type_key.to_string());
        let mut memo = BTreeMap::new();
        let mut in_progress = BTreeSet::new();
        let linear = match self.python_linearize(&root, 0, &mut memo, &mut in_progress) {
            Ok(linear) => linear,
            Err(MroStop::NotScoped) => return MroAnswer::NotScoped,
            Err(MroStop::Abstain) => return MroAnswer::Abstain,
        };
        for node in linear.iter().skip(1) {
            match node {
                MroNode::Opaque(_) => return MroAnswer::Abstain,
                MroNode::Class(file, identity) => {
                    let key = Self::python_type_key(identity);
                    match self.python_own_methods(file, key, method).as_slice() {
                        [] => {}
                        [(path, symbol)] => {
                            return MroAnswer::Found(PythonMethod {
                                file: path.clone(),
                                symbol: symbol.clone(),
                                declaring_type: key.to_string(),
                            });
                        }
                        _ => return MroAnswer::Abstain,
                    }
                }
            }
        }
        MroAnswer::Abstain
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
