//! A reference in type or heritage position, answered by a declaration in its
//! own file.
//!
//! The same-file rung accepted a declaration only if it was class-like and the
//! file declared the name with exactly one kind. Both conditions are right for
//! one question and wrong for two shapes JavaScript and TypeScript code writes
//! constantly:
//!
//! * **`extends` takes an expression.** `class Admin extends Mixed` with
//!   `const Mixed = mixin(Base)` is the mixin idiom; the `Extends` reference
//!   found the `const`, was refused for being a variable, and no rung below
//!   could answer it.
//! * **The companion idiom declares one name twice.** `const User = z.object(…)`
//!   beside `type User = z.infer<typeof User>` gives `User` two kinds in one
//!   file, so "which kind is it?" abstained, and every annotation of `User` in
//!   that file had no target.
//!
//! The counter-cases hold the filter where it is right: an annotation and an
//! `implements` clause name a type, and a `const` alone is not one.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// `(edge kind, source symbol)` for every resolved, non-structural edge into
/// `target`.
fn readers_of(resolution: &ResolutionResult, target: &str) -> Vec<(EdgeKind, String)> {
    let mut readers: Vec<(EdgeKind, String)> = resolution
        .edges
        .iter()
        .filter(|edge| edge.target_symbol == target)
        .filter(|edge| {
            !matches!(
                edge.edge_kind,
                EdgeKind::Contains | EdgeKind::Defines | EdgeKind::MemberOf
            )
        })
        .filter(|edge| {
            !matches!(
                edge.resolution.as_deref(),
                Some(Resolution::Unresolved { .. } | Resolution::AmbiguousGlobal { .. })
            )
        })
        .map(|edge| (edge.edge_kind, edge.source_symbol.clone()))
        .collect();
    readers.sort_by(|a, b| a.1.cmp(&b.1));
    readers
}

fn has(readers: &[(EdgeKind, String)], kind: EdgeKind, source: &str) -> bool {
    readers
        .iter()
        .any(|(edge_kind, from)| *edge_kind == kind && from == source)
}

#[test]
fn a_class_extending_a_const_extends_it() {
    for (path, source) in [
        (
            "src/mixins.js",
            "export const Mixed = mixin(Object);\nexport class Admin extends Mixed {}\n",
        ),
        (
            "src/mixins.ts",
            "export const Mixed = mixin(Object);\nexport class Admin extends Mixed {}\n",
        ),
    ] {
        let resolution = resolve(&[(path, source)]);
        let readers = readers_of(&resolution, &format!("{path}::Mixed"));
        assert!(
            has(&readers, EdgeKind::Extends, &format!("{path}::Admin")),
            "{path}: `extends Mixed` names the const: {readers:?}"
        );
    }
}

/// A second `User` elsewhere is what makes this the same-file rung's question:
/// with one declaration in the corpus the global tier answers it, and with two
/// — any application with a model and a schema — the global tier abstains and
/// the annotation's own file is the only evidence left.
#[test]
fn a_companion_type_is_reached_by_its_annotations() {
    let resolution = resolve(&[
        (
            "src/user.ts",
            "import { z } from \"zod\";\n\
             export const User = z.object({ name: z.string() });\n\
             export type User = z.infer<typeof User>;\n\
             export function load(raw: unknown): User { return User.parse(raw); }\n\
             export function show(u: User): string { return u.name; }\n",
        ),
        ("src/models/user.ts", "export class User { name = \"\"; }\n"),
    ]);
    let readers = readers_of(&resolution, "src/user.ts::User");
    for reader in ["src/user.ts::load", "src/user.ts::show"] {
        assert!(
            readers.iter().any(|(_, from)| from == reader),
            "{reader} annotates User: {readers:?}"
        );
    }
    let model = readers_of(&resolution, "src/models/user.ts::User");
    assert!(
        model
            .iter()
            .all(|(_, from)| !from.starts_with("src/user.ts")),
        "the model class is not the one src/user.ts annotates with: {model:?}"
    );
}

/// The companion's value half is read too — `User.parse(raw)`, or the schema
/// handed on as a value — and that read stopped at the same abstention: the
/// file declared `User` with two kinds, so the rung refused a plain name read
/// as well as an annotation.
#[test]
fn a_companion_const_is_reached_by_a_value_read() {
    let resolution = resolve(&[
        (
            "src/user.ts",
            "import { z } from \"zod\";\n\
             export const User = z.object({ name: z.string() });\n\
             export type User = z.infer<typeof User>;\n\
             export function schema() { return User; }\n",
        ),
        ("src/models/user.ts", "export class User { name = \"\"; }\n"),
    ]);
    let same_file: Vec<String> = resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.target_symbol == "src/user.ts::User"
                && edge.source_symbol == "src/user.ts::schema"
                && matches!(
                    edge.resolution.as_deref(),
                    Some(Resolution::SameFile { .. })
                )
        })
        .map(|edge| format!("{:?}", edge.edge_kind))
        .collect();
    assert!(
        !same_file.is_empty(),
        "`return User` reads the const its own file declares: {:?}",
        readers_of(&resolution, "src/user.ts::User")
    );
}

/// TypeScript keeps types and values apart: an annotation naming a `const`
/// that has no type of the same name names nothing in this file.
#[test]
fn an_annotation_never_names_a_lone_const() {
    let path = "src/config.ts";
    let resolution = resolve(&[(
        path,
        "export const Config = { debug: true };\n\
         export function read(c: Config): boolean { return true; }\n",
    )]);
    let readers = readers_of(&resolution, "src/config.ts::Config");
    assert!(
        !readers
            .iter()
            .any(|(_, from)| from == "src/config.ts::read"),
        "`c: Config` cannot name the const: {readers:?}"
    );
}

/// `implements` names a type, so it is `HeritageInterface` and keeps the
/// filter that `extends` no longer has.
#[test]
fn implements_never_names_a_const() {
    let path = "src/shape.ts";
    let resolution = resolve(&[(
        path,
        "export const Shape = { area: 0 };\nexport class Square implements Shape { area = 1; }\n",
    )]);
    let readers = readers_of(&resolution, "src/shape.ts::Shape");
    assert!(
        !has(&readers, EdgeKind::Implements, "src/shape.ts::Square"),
        "`implements Shape` cannot name the const: {readers:?}"
    );
}
