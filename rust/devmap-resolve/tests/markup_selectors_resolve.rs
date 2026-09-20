//! A DOM or stylesheet identity resolves against markup, and against nothing
//! else.
//!
//! `devmap_extract::markup` puts two new declaration kinds and one new reference
//! kind in the graph. The reference kind is the dangerous one: its names live in
//! a different namespace from identifiers, and the code resolution ladder does
//! not know that. Its unique-global rung answers a bare name with the single
//! declaration of it anywhere in the family — so a class called `menu` reaching
//! that rung binds to `function menu`, at `HIGH` confidence, and the graph then
//! asserts a relationship between a stylesheet and a function that has nothing
//! to do with it.
//!
//! That is the same defect class `embedded_script_families.rs` records — a
//! Svelte function bound to a Solidity method at 0.9 because both languages
//! shared one namespace — arriving by a different route, so the negative
//! assertions here are the point of the file. A missing edge is a gap a reader
//! can see. A fabricated one is a claim that a relationship exists which does
//! not.

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn is_selector_name(qualified: &str) -> bool {
    let name = qualified.rsplit("::").next().unwrap_or("");
    name.starts_with('.')
        || name.starts_with('#')
        || name.starts_with('[')
        || name.starts_with("--")
}

/// `References` edges whose target is a selector-shaped name — the *uses*.
///
/// `EdgeKind::Contains` is excluded deliberately, and not because it is noise:
/// the resolver emits one from a file to each symbol the file declares, which is
/// what makes a new declaration addressable in the graph at all
/// (`a_declaration_is_contained_by_its_file` below pins it). It is excluded
/// because its source is always the file and its target always a declaration in
/// that same file, so a `find` over both kinds silently answers questions about
/// *use* with the containment edge — which is how the first draft of this file
/// "passed" an assertion about which function owns a use.
fn selector_uses(resolution: &ResolutionResult) -> Vec<&devmap_resolve::model::ResolvedEdge> {
    resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::References && is_selector_name(&edge.target_symbol)
        })
        .collect()
}

/// A new declaration is reachable from its file, like every other declaration.
#[test]
fn a_declaration_is_contained_by_its_file() {
    let (_, resolution) = resolve(&[("src/Board.svelte", COMPONENT)]);
    let contained: Vec<&str> = resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Contains && is_selector_name(&edge.target_symbol)
        })
        .map(|edge| edge.target_symbol.as_str())
        .collect();
    for expected in [
        "src/Board.svelte::[data-hook]",
        "src/Board.svelte::.nav-heading",
    ] {
        assert!(
            contained.contains(&expected),
            "{expected} must be contained by its file, or nothing can navigate to it: \
             {contained:?}"
        );
    }
}

const COMPONENT: &str =
    "<script>\n  function toggle() { document.querySelector(\"[data-hook]\"); }\n\
                         </script>\n\
                         <div class=\"nav-heading\" data-hook>x</div>\n\
                         <style>\n  .nav-heading { color: red; }\n</style>\n";

/// A component's own `<style>` answers its own `class` attribute.
///
/// `SameFile`, because a component's stylesheet is scoped to it by every
/// framework that has one — the same evidence tier the code ladder gives a
/// declaration in the file that names it.
#[test]
fn a_class_attribute_resolves_to_the_rule_in_the_same_file() {
    let (_, resolution) = resolve(&[("src/Board.svelte", COMPONENT)]);
    let edge = selector_uses(&resolution)
        .into_iter()
        .find(|edge| edge.target_symbol.ends_with("::.nav-heading"))
        .expect("the class attribute must reach the rule that declares it");
    assert_eq!(edge.target_file, "src/Board.svelte");
    assert_eq!(edge.edge_kind, EdgeKind::References);
    assert_eq!(
        edge.confidence,
        Confidence::DETERMINISTIC,
        "a declaration in this very file is as certain as evidence gets"
    );
    assert!(
        matches!(
            edge.resolution.as_deref(),
            Some(Resolution::SameFile { .. })
        ),
        "the evidence must name the rung: {:?}",
        edge.resolution
    );
}

/// The reported case, end to end through the resolver: a selector string in the
/// script reaches the hook the markup declares.
#[test]
fn a_selector_string_reaches_the_hook_the_markup_declares() {
    let (_, resolution) = resolve(&[("src/Board.svelte", COMPONENT)]);
    let edge = selector_uses(&resolution)
        .into_iter()
        .find(|edge| edge.target_symbol.ends_with("::[data-hook]"))
        .expect("the querySelector string must reach the element's hook");
    assert!(
        edge.source_symbol.ends_with("::toggle"),
        "the edge starts at the function that queries it, not at the file: {}",
        edge.source_symbol
    );
}

/// A global stylesheet's rule answers every component that names it.
#[test]
fn a_global_stylesheet_rule_answers_a_component_that_names_it() {
    let (_, resolution) = resolve(&[
        ("src/app.css", ".btn { padding: 2px; }\n"),
        ("src/Save.svelte", "<button class=\"btn\">Save</button>\n"),
    ]);
    let edge = selector_uses(&resolution)
        .into_iter()
        .find(|edge| {
            edge.source_file == "src/Save.svelte" && edge.target_symbol.ends_with("::.btn")
        })
        .expect("the component must reach the one stylesheet that declares .btn");
    assert_eq!(edge.target_file, "src/app.css");
    assert_eq!(
        edge.confidence,
        Confidence::HIGH,
        "one declaration of the name, with nothing tying it to this file: strong, not certain"
    );
    assert!(
        matches!(
            edge.resolution.as_deref(),
            Some(Resolution::UniqueSelector { .. })
        ),
        "the evidence must say it was the only declaration: {:?}",
        edge.resolution
    );
}

/// Two components that each scope a `.card` are two different `.card`s.
///
/// The ambiguity that must produce **no** edge rather than a guess. Picking
/// either would assert that one component's markup depends on another's
/// stylesheet, which is exactly false in a scoped-styles framework.
#[test]
fn a_class_two_components_each_scope_resolves_to_neither() {
    let (_, resolution) = resolve(&[
        (
            "src/A.svelte",
            "<div class=\"card\">a</div>\n<style>.card { color: red; }</style>\n",
        ),
        (
            "src/B.svelte",
            "<div class=\"card\">b</div>\n<style>.card { color: blue; }</style>\n",
        ),
        ("src/C.svelte", "<div class=\"card\">c</div>\n"),
    ]);
    // A and B each answer themselves.
    for file in ["src/A.svelte", "src/B.svelte"] {
        let edge = selector_uses(&resolution)
            .into_iter()
            .find(|edge| edge.source_file == file && edge.target_symbol.ends_with("::.card"))
            .unwrap_or_else(|| panic!("{file} declares .card and must answer its own use"));
        assert_eq!(
            edge.target_file, file,
            "a scoped rule answers its own component, not the other one"
        );
    }
    // C declares nothing, and two candidates cannot be narrowed to one.
    let from_c: Vec<&str> = selector_uses(&resolution)
        .into_iter()
        .filter(|edge| edge.source_file == "src/C.svelte")
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert!(
        from_c.is_empty(),
        "two components scope .card, so C's use resolves to neither, not to one: {from_c:?}"
    );
}

/// A component's scoped `<style>` cannot style another component.
///
/// Uniqueness is not enough for a cross-file selector edge, which is what
/// separates this rung from the code ladder's. Svelte, Vue and Astro each compile
/// a component's `<style>` to rules that match only that component's own
/// elements, so a `.selected` declared in exactly one component is still
/// unreachable from a different one — and an edge saying otherwise is false, not
/// merely uncertain.
///
/// Measured on GitPulse: of 672 cross-file selector edges, 668 pointed at the
/// global `src/app.css` and were right; three of the other four were exactly this
/// shape — `.selected`, `.sheet-body` and `.status`, each declared in one
/// component's scoped stylesheet and named by another's markup.
#[test]
fn a_class_scoped_to_one_component_does_not_answer_another() {
    let (_, resolution) = resolve(&[
        (
            "src/TaskBoard.svelte",
            "<div class=\"selected\">a</div>\n<style>.selected { color: red; }</style>\n",
        ),
        ("src/BranchList.svelte", "<div class=\"selected\">b</div>\n"),
    ]);
    let crossing: Vec<(&str, &str)> = selector_uses(&resolution)
        .into_iter()
        .filter(|edge| edge.source_file != edge.target_file)
        .map(|edge| (edge.source_file.as_str(), edge.target_symbol.as_str()))
        .collect();
    assert!(
        crossing.is_empty(),
        "a scoped rule in one component answered another component: {crossing:?}"
    );
    // The component that declares it still answers itself.
    assert!(
        selector_uses(&resolution)
            .iter()
            .any(|edge| edge.source_file == "src/TaskBoard.svelte"
                && edge.target_file == "src/TaskBoard.svelte"),
        "the declaring component's own use still resolves"
    );
}

/// An id is unique per *document*, so it crosses components.
///
/// The case the scoping rule must not break: `aria-controls` naming an element
/// another component renders is how an ARIA relationship is written across a
/// page, and it was one of the four cross-file edges on the real corpus that was
/// correct.
#[test]
fn an_id_declared_in_another_component_is_still_reachable() {
    let (_, resolution) = resolve(&[
        (
            "src/TaskArchive.svelte",
            "<div id=\"task-archive-dock\">dock</div>\n",
        ),
        (
            "src/TaskBoard.svelte",
            "<button aria-controls=\"task-archive-dock\">open</button>\n",
        ),
    ]);
    let edge = selector_uses(&resolution)
        .into_iter()
        .find(|edge| {
            edge.source_file == "src/TaskBoard.svelte"
                && edge.target_symbol.ends_with("::#task-archive-dock")
        })
        .expect("an id is document-global, so the ARIA relationship is a real edge");
    assert_eq!(edge.target_file, "src/TaskArchive.svelte");
    assert_eq!(
        edge.confidence,
        Confidence::HIGH,
        "one declaration of the id, and nothing proving both components share a document"
    );
}

/// A global stylesheet and a component's markup can declare the same name, and
/// no edge is asserted between them.
///
/// The honest limit, and a claim this file first made the other way round. A
/// stylesheet's selectors are read as *declarations*, so `[data-add-repo]` in
/// `app.css` and `data-add-repo` on an element are two declarations of one name
/// rather than a use and a declaration — and two declarations leave the
/// cross-file rung nothing to choose between. Both are findable by search, which
/// is what the reported gap asked for; the edge between them is not asserted,
/// which is the conservative direction.
///
/// Writing this test the other way is what showed that a third cross-file clause
/// — *the reference is global* — had no correct producer, and it was removed.
#[test]
fn a_stylesheet_and_markup_may_both_declare_a_name_without_an_edge() {
    let (extractions, resolution) = resolve(&[
        ("src/app.css", "[data-add-repo] { display: flex; }\n"),
        ("src/TaskBoard.svelte", "<div data-add-repo>x</div>\n"),
    ]);
    let declaring: Vec<&str> = extractions
        .iter()
        .filter(|extraction| {
            extraction
                .symbols
                .iter()
                .any(|symbol| symbol.name == "[data-add-repo]")
        })
        .map(|extraction| extraction.file_path.as_str())
        .collect();
    assert_eq!(
        declaring,
        vec!["src/app.css", "src/TaskBoard.svelte"],
        "each file declares the name it carries, so a search for it finds both"
    );
    let crossing: Vec<(&str, &str)> = selector_uses(&resolution)
        .into_iter()
        .filter(|edge| edge.source_file != edge.target_file)
        .map(|edge| (edge.source_file.as_str(), edge.target_symbol.as_str()))
        .collect();
    assert!(
        crossing.is_empty(),
        "two declarations leave nothing to choose between, so no edge is asserted: {crossing:?}"
    );
}

/// A class is never a function, whatever both are called.
#[test]
fn a_selector_never_resolves_to_a_code_symbol() {
    let (_, resolution) = resolve(&[
        (
            "src/menu.ts",
            "export function menu(): number { return 1; }\n",
        ),
        ("src/Nav.svelte", "<div class=\"menu\" data-menu>x</div>\n"),
    ]);
    for edge in &resolution.edges {
        let target = edge.target_symbol.rsplit("::").next().unwrap_or("");
        if edge.source_file != "src/Nav.svelte" {
            continue;
        }
        assert_ne!(
            target, "menu",
            "a class named `menu` bound to the function `menu`: {edge:?}"
        );
    }
}

/// A selector that resolves to nothing is not filed as a failed code
/// attribution.
///
/// The unresolved ledger is the tier documented as the one that indicates a
/// defect. A class whose rule lives in a global stylesheet outside the tree, in
/// a framework, or behind a CDN is the ordinary case for a frontend — filing
/// each of those as a failed attribution would put tens of thousands of
/// non-defects in the one list a maintainer is meant to read.
#[test]
fn an_unresolvable_selector_is_not_an_unresolved_code_reference() {
    let (_, resolution) = resolve(&[(
        "src/Nav.svelte",
        "<div class=\"from-bootstrap btn-lg\" aria-controls=\"elsewhere\">x</div>\n",
    )]);
    for row in &resolution.unresolved {
        assert!(
            !row.callee_name.starts_with('.')
                && !row.callee_name.starts_with('#')
                && !row.callee_name.starts_with('[')
                && !row.callee_name.starts_with("--"),
            "a selector reached the unresolved ledger: {row:?}"
        );
    }
}

/// Resolution is deterministic, including for the cross-file rung.
///
/// The cross-file rung reads a list of declaring files and takes the single
/// entry; if that list were built from an unordered collection, the *same* corpus
/// could resolve differently between builds once a second declaration appeared
/// and disappeared. Two resolutions of one corpus must agree edge for edge.
#[test]
fn resolving_the_same_corpus_twice_gives_the_same_edges() {
    let corpus: &[(&str, &str)] = &[
        (
            "src/app.css",
            ".btn { padding: 2px; }\n.card { margin: 0; }\n",
        ),
        (
            "src/Save.svelte",
            "<button class=\"btn card\">Save</button>\n",
        ),
        ("src/Board.svelte", COMPONENT),
    ];
    let fingerprint = |resolution: &ResolutionResult| {
        let mut rows: Vec<String> = selector_uses(resolution)
            .into_iter()
            .map(|edge| {
                format!(
                    "{} {} -> {} {} {:?}",
                    edge.source_file,
                    edge.source_symbol,
                    edge.target_file,
                    edge.target_symbol,
                    edge.confidence
                )
            })
            .collect();
        rows.sort();
        rows
    };
    let (_, first) = resolve(corpus);
    let (_, second) = resolve(corpus);
    assert_eq!(fingerprint(&first), fingerprint(&second));
    assert!(
        !fingerprint(&first).is_empty(),
        "this test must have had edges to compare"
    );
}
