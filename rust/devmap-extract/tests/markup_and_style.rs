//! The markup and stylesheet halves of a file are indexed.
//!
//! The reported gap, verbatim: on a current index of a real Svelte application,
//! `devmap search toggleAddMenu` returned the `<script>` function while
//! `devmap search data-add-repo` and `devmap search nav-heading` returned 0
//! hits with `truncated=false` — an empty answer, on a built index, about two
//! names the file plainly declares. The markup and `<style>` halves of every
//! template file, and the whole of every standalone `.css` and `.html` file,
//! were indexed nowhere.
//!
//! These tests are written against the shape of that file rather than a
//! minimal one, because the defect was not in a corner: it was the ordinary
//! case.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, ReferenceKind, SymbolKind};

/// The reported file, reduced to the three things it proved were missing: a
/// valueless `data-*` hook, a selector string in the script that targets it,
/// and a class declared by `<style>` and used by a `class` attribute.
const TASK_BOARD: &str = r#"<script lang="ts">
  let addMenu = false;
  const outside = { inside: "[data-add-repo], [data-add-repo-popup]" };
  function toggleAddMenu() {
    addMenu = !addMenu;
    document.querySelector(".nav-heading")?.classList.add("open");
  }
</script>

<div class="nav-heading wide" data-add-repo id="repo-heading">
  <button type="button" class="icon" on:click={toggleAddMenu} aria-controls="task-add-repo-menu">Add</button>
  {#if addMenu}
    <div class="menu" data-add-repo-popup id="task-add-repo-menu" class:open={addMenu}>Menu</div>
  {/if}
</div>

<style>
  .nav-heading { display: flex; --gap-x: 4px; }
  .nav-heading > .icon:hover { opacity: var(--gap-x); }
  #repo-heading, div.menu[data-add-repo-popup] { color: red; }
  @keyframes pulse { from { opacity: 0; } to { opacity: 1; } }
  @media (max-width: 40rem) { .menu { display: none; } }
</style>
"#;

fn names_of_kind(extraction: &Extraction, kind: SymbolKind) -> Vec<String> {
    let mut names: Vec<String> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == kind)
        .map(|symbol| symbol.name.clone())
        .collect();
    names.sort();
    names
}

fn selector_uses(extraction: &Extraction) -> Vec<String> {
    let mut names: Vec<String> = extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Selector)
        .map(|reference| reference.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn a_svelte_component_declares_its_dom_hooks() {
    let extraction = extract_file("src/lib/components/TaskBoard.svelte", TASK_BOARD);
    let anchors = names_of_kind(&extraction, SymbolKind::MarkupAnchor);
    assert_eq!(
        anchors,
        vec![
            "#repo-heading",
            "#task-add-repo-menu",
            "[data-add-repo-popup]",
            "[data-add-repo]",
        ],
        "every id and data-* hook in the markup is a declaration"
    );
}

#[test]
fn a_svelte_component_declares_its_style_rules() {
    let extraction = extract_file("src/lib/components/TaskBoard.svelte", TASK_BOARD);
    let rules = names_of_kind(&extraction, SymbolKind::StyleRule);
    // `[data-add-repo-popup]` is declared by the markup as an anchor and by the
    // stylesheet as a rule component. One declaration per name per file, so the
    // markup's (which comes first) is the one that stands.
    assert!(
        rules.contains(&"@keyframes pulse".to_string()),
        "a keyframes name is a declaration: {rules:?}"
    );
    assert!(
        rules.contains(&"--gap-x".to_string()),
        "a custom property is a declaration: {rules:?}"
    );
    assert!(
        rules.contains(&".nav-heading".to_string()),
        "a class selector is a declaration: {rules:?}"
    );
    assert!(
        rules.contains(&".icon".to_string()),
        "a class in a descendant selector is a declaration: {rules:?}"
    );
    assert!(
        rules.contains(&".menu".to_string()),
        "a class nested inside @media is a declaration: {rules:?}"
    );
}

#[test]
fn the_script_half_and_the_markup_half_are_joined() {
    let extraction = extract_file("src/lib/components/TaskBoard.svelte", TASK_BOARD);
    let uses = selector_uses(&extraction);
    assert!(
        uses.contains(&"[data-add-repo]".to_string()),
        "the selector string in the script is a use of the markup's hook: {uses:?}"
    );
    assert!(
        uses.contains(&"[data-add-repo-popup]".to_string()),
        "both hooks in one selector string are read: {uses:?}"
    );
    assert!(
        uses.contains(&".nav-heading".to_string()),
        "the class attribute and the querySelector argument are both uses: {uses:?}"
    );
    assert!(
        uses.contains(&"#task-add-repo-menu".to_string()),
        "aria-controls is an IDREF attribute, so it names an id: {uses:?}"
    );
    assert!(
        uses.contains(&".open".to_string()),
        "a Svelte class: directive names a class: {uses:?}"
    );
    assert!(
        uses.contains(&"--gap-x".to_string()),
        "var(--gap-x) is a read of the custom property: {uses:?}"
    );
}

/// A use inside a function is that function's use.
#[test]
fn a_selector_string_is_attributed_to_the_function_it_sits_in() {
    let extraction = extract_file("src/lib/components/TaskBoard.svelte", TASK_BOARD);
    let attributed: Vec<(String, String)> = extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Selector)
        .filter_map(|reference| {
            reference
                .enclosing_symbol
                .clone()
                .map(|owner| (reference.name.clone(), owner))
        })
        .collect();
    assert!(
        attributed
            .iter()
            .any(|(name, owner)| name == ".nav-heading" && owner.ends_with("::toggleAddMenu")),
        "the querySelector call inside toggleAddMenu belongs to it: {attributed:?}"
    );
    assert!(
        attributed
            .iter()
            .any(|(name, owner)| name == "[data-add-repo]"
                && owner == "src/lib/components/TaskBoard.svelte"),
        "a use at module scope belongs to the file: {attributed:?}"
    );
}

/// Every span must slice its own name out of the file on disk.
///
/// A span is a byte range into the file and every consumer renders a symbol by
/// slicing the file with it, so a span shifted by a region offset is not a
/// coarser answer but a wrong one. This is the assertion that catches an
/// unshifted `<style>` span, which is the whole reason the stylesheet reader
/// takes a base offset.
#[test]
fn every_span_slices_the_name_it_claims() {
    let extraction = extract_file("src/lib/components/TaskBoard.svelte", TASK_BOARD);
    // Counted, because a loop over an empty list passes every assertion inside
    // it. With the merge disabled this test was one of two in the file that
    // still passed, which made it a test of nothing.
    let mut checked_symbols = 0usize;
    let mut checked_references = 0usize;
    for symbol in &extraction.symbols {
        if !matches!(
            symbol.kind,
            SymbolKind::MarkupAnchor | SymbolKind::StyleRule
        ) {
            continue;
        }
        let text = &TASK_BOARD[symbol.span.start_byte..symbol.span.end_byte];
        let expected = symbol
            .name
            .trim_start_matches(['.', '#'])
            .trim_start_matches("@keyframes ")
            .trim_start_matches('[')
            .trim_end_matches(']');
        assert!(
            text.contains(expected),
            "{} span [{}..{}] reads {text:?}, which does not contain {expected:?}",
            symbol.name,
            symbol.span.start_byte,
            symbol.span.end_byte
        );
        checked_symbols += 1;
    }
    for reference in &extraction.references {
        if reference.kind != ReferenceKind::Selector {
            continue;
        }
        let text = &TASK_BOARD[reference.span.start_byte..reference.span.end_byte];
        let expected = reference
            .name
            .trim_start_matches(['.', '#'])
            .trim_start_matches('[')
            .trim_end_matches(']');
        assert!(
            text.contains(expected),
            "use of {} at [{}..{}] reads {text:?}",
            reference.name,
            reference.span.start_byte,
            reference.span.end_byte
        );
        checked_references += 1;
    }
    assert!(
        checked_symbols >= 7 && checked_references >= 6,
        "this test must have had something to check: {checked_symbols} declarations and \
         {checked_references} uses"
    );
}

/// The code half must be unchanged: this pass adds, it does not replace.
#[test]
fn the_script_half_still_extracts_exactly_as_before() {
    let extraction = extract_file("src/lib/components/TaskBoard.svelte", TASK_BOARD);
    assert!(
        extraction
            .symbols
            .iter()
            .any(|symbol| symbol.name == "toggleAddMenu" && symbol.kind == SymbolKind::Function),
        "the <script> function is still a Function"
    );
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "nothing was left unread, so the outcome is still Clean: {:?}",
        extraction.parse_outcome
    );
}

/// Every template language, plus the two whole-file languages.
///
/// The four template grammars model markup to different depths — Svelte, Vue and
/// Astro give `attribute` nodes; Liquid hands back one opaque `template_content`
/// leaf — and `.html`/`.css` have no grammar at all. One table, so a language
/// that reads its script half but not its markup half cannot hide.
#[test]
fn every_markup_language_reads_its_markup_half() {
    let cases: &[(&str, &str)] = &[
        (
            "a.svelte",
            "<div class=\"card\" data-hook id=\"top\">x</div>\n<style>.card { color: red; }</style>\n",
        ),
        (
            "a.vue",
            "<template>\n<div class=\"card\" data-hook id=\"top\">x</div>\n</template>\n<style scoped>.card { color: red; }</style>\n",
        ),
        (
            "a.astro",
            "---\nconst x = 1;\n---\n<div class=\"card\" data-hook id=\"top\">x</div>\n<style>.card { color: red; }</style>\n",
        ),
        (
            "a.liquid",
            "{% if x %}<div class=\"card\" data-hook id=\"top\">x</div>{% endif %}\n<style>.card { color: red; }</style>\n",
        ),
        (
            "a.html",
            "<!doctype html>\n<div class=\"card\" data-hook id=\"top\">x</div>\n<style>.card { color: red; }</style>\n",
        ),
    ];
    for (path, source) in cases {
        let extraction = extract_file(path, source);
        let anchors = names_of_kind(&extraction, SymbolKind::MarkupAnchor);
        assert!(
            anchors.contains(&"[data-hook]".to_string()),
            "{path}: a data-* hook must be a declaration, got {anchors:?}"
        );
        assert!(
            anchors.contains(&"#top".to_string()),
            "{path}: an id must be a declaration, got {anchors:?}"
        );
        let rules = names_of_kind(&extraction, SymbolKind::StyleRule);
        assert!(
            rules.contains(&".card".to_string()),
            "{path}: the <style> block's rule must be a declaration, got {rules:?}"
        );
        let uses = selector_uses(&extraction);
        assert!(
            uses.contains(&".card".to_string()),
            "{path}: the class attribute must be a use, got {uses:?}"
        );
    }
}

/// An attribute name is case-insensitive; a class, an id and a custom property
/// are not.
///
/// Found by the fuzz corpus in `markup_under_stress.rs`, which emitted
/// `[data-xzclass]` for a file containing `data-xZclass`: the markup reader
/// folded an attribute name and the stylesheet reader did not. The graph held a
/// name that was not in the file, and — worse than cosmetic — the two halves of
/// the same contract stopped joining while each looked correct on its own.
///
/// The rule is the specifications': HTML matches attribute names ASCII
/// case-insensitively, so `data-Foo` and `[data-foo]` are one name; class names
/// and ids are case-sensitive, so `.Card` and `.card` are two.
#[test]
fn an_attribute_name_folds_case_and_a_class_does_not() {
    let source = "<script>\n  function go() { q(\"[data-Foo]\"); }\n</script>\n\
                  <div data-Foo class=\"Card card\" id=\"Top\" class:isActive={on}>x</div>\n\
                  <style>\n  [data-FOO] { x: 1 }\n  .Card { y: 1 }\n  .card { z: 1 }\n\
                  #Top { w: 1 }\n</style>\n";
    let extraction = extract_file("a.svelte", source);
    let anchors = names_of_kind(&extraction, SymbolKind::MarkupAnchor);
    assert!(
        anchors.contains(&"[data-foo]".to_string()),
        "the attribute name is folded once, to lowercase: {anchors:?}"
    );
    assert!(
        anchors.contains(&"#Top".to_string()),
        "an id keeps its case, because HTML ids are case-sensitive: {anchors:?}"
    );

    let rules = names_of_kind(&extraction, SymbolKind::StyleRule);
    assert!(
        rules.contains(&".Card".to_string()) && rules.contains(&".card".to_string()),
        "two classes differing only in case are two declarations: {rules:?}"
    );
    // `[data-FOO]` in the rule folds to the same name the markup declared, so it
    // is the same declaration seen again rather than a second one.
    assert!(
        !rules.contains(&"[data-FOO]".to_string()),
        "an attribute selector must not keep its case, or it joins nothing: {rules:?}"
    );

    let uses = selector_uses(&extraction);
    assert!(
        uses.contains(&".isActive".to_string()),
        "a class: directive keeps the class name's case: {uses:?}"
    );
    assert!(
        uses.contains(&"[data-foo]".to_string()),
        "the folded attribute name is what the script's selector string joins to: {uses:?}"
    );
    assert!(
        uses.contains(&".Card".to_string()) && uses.contains(&".card".to_string()),
        "both spellings in the class attribute are distinct uses: {uses:?}"
    );
}

/// A page or stylesheet that declares nothing is a completed read, not a
/// failure.
///
/// The regression this change caused and the store's K5 test caught. Giving
/// `html` and `css` a reader moved them out of the "declares nothing" list, and
/// a page with no ids then landed in the arm that means *a grammar was wanted for
/// this language and was not there* — so every `.html` and `.css` file without an
/// anchor or a rule counted as a parse failure. That is the shape which once put
/// 294 of 1,310 files in that count and buried the 16 real ones.
///
/// Asserted here as well as in the store, because this is the crate that decides
/// it and a file-level fact should fail in the crate that produced it.
#[test]
fn a_page_or_stylesheet_with_nothing_to_declare_is_not_a_parse_failure() {
    for (path, source) in [
        ("page.html", "<html><body>hi</body></html>\n"),
        ("empty.css", "/* nothing here */\n"),
        ("blank.html", "\n"),
        ("blank.css", ""),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.is_parse_failure(),
            "{path} declares nothing and the reader read it; that is not a failure \
             (outcome {:?}, engine {:?})",
            extraction.parse_outcome,
            extraction.engine
        );
        assert!(
            extraction
                .symbols
                .iter()
                .any(|symbol| symbol.kind == SymbolKind::File),
            "{path} must still be addressable as a File node"
        );
    }

    // And a file that *does* declare something still reports what it found.
    let extraction = extract_file("page.html", "<div id=\"root\"></div>\n");
    assert!(!extraction.is_parse_failure());
    assert_eq!(
        names_of_kind(&extraction, SymbolKind::MarkupAnchor),
        vec!["#root"]
    );
}

#[test]
fn a_standalone_stylesheet_declares_its_rules() {
    for path in ["src/app.css", "src/app.scss", "src/app.less"] {
        let extraction = extract_file(
            path,
            ".btn, .btn-primary { --pad: 2px; color: var(--fg); }\n#root { margin: 0; }\n",
        );
        let rules = names_of_kind(&extraction, SymbolKind::StyleRule);
        assert_eq!(
            rules,
            vec!["#root", "--pad", ".btn", ".btn-primary"],
            "{path} declares every selector in the list and its custom property"
        );
        assert!(
            selector_uses(&extraction).contains(&"--fg".to_string()),
            "{path}: var(--fg) is a use"
        );
    }
}
