//! Which declaration a member of a JS/TS module-scope object is.
//!
//! `export const debugLogger = { ...core, clear: clearLogs }` has members its
//! literal does not spell (`core`'s) and members whose value is another
//! declaration (`clearLogs`). The extractor records how each const object is
//! built, in source order ([`JsObjectFact`]); this reads those steps the way
//! JavaScript applies them, last write first:
//!
//! * a key written in place, or forwarded from an identifier, is that
//!   declaration — an explicit key after a spread beats it, a later spread
//!   beats an earlier one;
//! * a spread, `Object.assign` source or alias is followed through the import
//!   binding to the source object's own steps, up to
//!   [`Resolver::REEXPORT_CHAIN_MAX_DEPTH`] hops;
//! * anything the extraction could not name — a spread of a call's result, a
//!   computed key, a source in a file that is not indexed — may define any key,
//!   so the walk abstains for every key it has not yet answered, and a cycle
//!   abstains too.
//!
//! An abstention is not "the key is absent": only a walk that reaches the first
//! step having met no unknown says [`ObjectMember::Absent`].

use super::*;

/// A write the extraction could not read, standing in for it in a step list.
static UNREADABLE_STEP: JsObjectPart = JsObjectPart::Unknown;

/// What a name used as a spread source, alias or receiver denotes.
enum ObjectRef {
    /// A module-scope const object this corpus declares.
    Object(String, String),
    /// A whole module (`import * as core`, `require('./core')`): its members
    /// are the file's own top-level declarations.
    Namespace(String),
    Unknown,
}

pub(super) enum ObjectMember {
    /// The member's declaration: `(file, symbol identity)`.
    Bound(String, String),
    /// No step defines it and every step was readable.
    Absent,
    /// A step that may define it could not be read.
    Abstain,
}

impl Resolver {
    pub(super) fn index_js_objects(&mut self, ext: &Extraction) {
        if !ext.js_objects.is_empty() {
            self.js_objects
                .insert(ext.file_path.clone(), ext.js_objects.clone());
        }
    }

    fn declared_object(&self, file: &str, name: &str) -> bool {
        self.js_objects
            .get(file)
            .is_some_and(|facts| facts.iter().any(|f| f.declaration && f.name == name))
    }

    /// A value `name` that module `module_file` publishes, as an object this
    /// corpus declares — directly or through a re-export chain.
    fn object_in_module(&self, module_file: &str, name: &str) -> ObjectRef {
        if self.declared_object(module_file, name) {
            return ObjectRef::Object(module_file.to_string(), name.to_string());
        }
        if let Some((file, declared)) = self
            .reexport_chains
            .get(&format!("{module_file}::{name}"))
            .and_then(|terminal| terminal.rsplit_once("::"))
        {
            if self.declared_object(file, declared) {
                return ObjectRef::Object(file.to_string(), declared.to_string());
            }
        }
        ObjectRef::Unknown
    }

    /// What `reference` — `name` or `ns.name`, as written in `file` — denotes.
    fn object_ref(&self, file: &str, reference: &str) -> ObjectRef {
        if let Some((root, member)) = reference.split_once('.') {
            if self.is_value_import(file, root) {
                return ObjectRef::Unknown;
            }
            return match self.import_bindings.get(file).and_then(|b| b.get(root)) {
                Some((module_file, _)) => self.object_in_module(module_file, member),
                None => ObjectRef::Unknown,
            };
        }
        if self.declared_object(file, reference) {
            return ObjectRef::Object(file.to_string(), reference.to_string());
        }
        if let Some((declaring_file, declared)) = self
            .value_imports
            .get(&(file.to_string(), reference.to_string()))
        {
            return self.object_in_module(declaring_file, declared);
        }
        match self
            .import_bindings
            .get(file)
            .and_then(|bindings| bindings.get(reference))
        {
            Some((module_file, _)) => ObjectRef::Namespace(module_file.clone()),
            None => ObjectRef::Unknown,
        }
    }

    /// The declaration a forwarded identifier names in `file`: a named import,
    /// or a top-level declaration of the file itself.
    fn forwarded_declaration(&self, file: &str, identifier: &str) -> ObjectMember {
        let named = match self
            .import_bindings
            .get(file)
            .and_then(|bindings| bindings.get(identifier))
        {
            Some((target_file, target_symbol)) => {
                self.lookup_in_package(target_file, target_symbol)
            }
            None => self.lookup_in_package(file, identifier),
        };
        match named {
            Some((found_file, symbol)) => ObjectMember::Bound(found_file, symbol),
            None => ObjectMember::Abstain,
        }
    }

    fn object_member_walk(
        &self,
        file: &str,
        value: &str,
        member: &str,
        trail: &mut Vec<(String, String)>,
    ) -> ObjectMember {
        let key = (file.to_string(), value.to_string());
        if trail.len() >= Self::REEXPORT_CHAIN_MAX_DEPTH || trail.contains(&key) {
            return ObjectMember::Abstain;
        }
        let Some(facts) = self.js_objects.get(file) else {
            return ObjectMember::Abstain;
        };
        let mut declarations = facts.iter().filter(|f| f.declaration && f.name == value);
        let (Some(declaration), None) = (declarations.next(), declarations.next()) else {
            // Two declarations of one name: which ran last is not a fact the
            // extraction holds.
            return ObjectMember::Abstain;
        };
        let mutations = facts
            .iter()
            .filter(|f| !f.declaration && f.name == value && f.start_byte > declaration.start_byte);
        // A write through an alias of this object (`const w = core;
        // Object.assign(w, …)`) lands in this object's members too.
        let written_through_alias = facts.iter().any(|alias| {
            alias.declaration
                && alias.alias_of.as_deref() == Some(value)
                && facts.iter().any(|f| !f.declaration && f.name == alias.name)
        });
        let mut steps: Vec<&JsObjectPart> = declaration.parts.iter().collect();
        for mutation in mutations {
            if mutation.in_callable {
                steps.push(&UNREADABLE_STEP);
            } else {
                steps.extend(mutation.parts.iter());
            }
        }
        if written_through_alias {
            steps.push(&UNREADABLE_STEP);
        }
        trail.push(key);
        let answer = self.apply_steps(file, value, member, &steps, trail);
        trail.pop();
        answer
    }

    fn apply_steps(
        &self,
        file: &str,
        value: &str,
        member: &str,
        steps: &[&JsObjectPart],
        trail: &mut Vec<(String, String)>,
    ) -> ObjectMember {
        for step in steps.iter().rev() {
            match step {
                JsObjectPart::Inline(key) if key == member => {
                    // The extractor files a member under its owner's own
                    // identity, which is the file-qualified name.
                    let qualified = format!("{file}::{value}.{member}");
                    let declared = self.symbol_index.get(&qualified).and_then(|hits| {
                        hits.iter()
                            .find(|(path, _, _, identity)| path == file && **identity == *qualified)
                    });
                    return match declared {
                        Some(_) => ObjectMember::Bound(file.to_string(), qualified),
                        None => ObjectMember::Abstain,
                    };
                }
                JsObjectPart::Forward {
                    key,
                    value: forwarded,
                } if key == member => {
                    return self.forwarded_declaration(file, forwarded);
                }
                JsObjectPart::Opaque(key) if key == member => return ObjectMember::Abstain,
                JsObjectPart::Spread(reference) => match self.object_ref(file, reference) {
                    ObjectRef::Object(source_file, source) => {
                        match self.object_member_walk(&source_file, &source, member, trail) {
                            ObjectMember::Absent => {}
                            decided => return decided,
                        }
                    }
                    ObjectRef::Namespace(module_file) => {
                        return match self.lookup_in_package(&module_file, member) {
                            Some((found_file, symbol)) => ObjectMember::Bound(found_file, symbol),
                            None => ObjectMember::Abstain,
                        };
                    }
                    ObjectRef::Unknown => return ObjectMember::Abstain,
                },
                JsObjectPart::Unknown => return ObjectMember::Abstain,
                _ => {}
            }
        }
        ObjectMember::Absent
    }

    /// The declaration a call `receiver.member()` written in `file` reaches
    /// when `receiver` is a module-scope const object built in place, by
    /// spread, by `Object.assign` or as an alias, in this file or another.
    pub(super) fn object_member_target(
        &self,
        file: &str,
        receiver: &str,
        member: &str,
    ) -> Option<(String, String)> {
        let ObjectRef::Object(object_file, object) = self.object_ref(file, receiver) else {
            return None;
        };
        match self.object_member_walk(&object_file, &object, member, &mut Vec::new()) {
            ObjectMember::Bound(found_file, symbol) => Some((found_file, symbol)),
            ObjectMember::Absent | ObjectMember::Abstain => None,
        }
    }
}
