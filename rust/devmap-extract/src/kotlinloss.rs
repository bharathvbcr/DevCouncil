//! The Kotlin silent declaration drop (extract-v98 row 15): the loss detector.
//!
//! tree-sitter-kotlin-ng parses some declarations as *expressions* and reports
//! no error. After two or more annotations and one modifier-like word,
//! `private fun f() {}` is `annotated_expression > annotated_expression >
//! infix_expression [identifier private, identifier fun, call_expression ...]`:
//! a clean tree in which the declaration does not exist. The file was published
//! `Clean`, so no consumer could tell that `f` had been dropped, and a caller of
//! `f` read as a call to nothing. The same shape swallows classes, objects,
//! interfaces, properties and type aliases, and `fun f() = 1` arrives as an
//! `assignment` around the same expression.
//!
//! # The detector is a tree check
//!
//! `fun`, `class`, `interface`, `object`, `val`, `var` and `typealias` are hard
//! keywords: valid Kotlin never spells one as an `identifier`, so an `identifier`
//! node whose whole text is one of them is the grammar having read a declaration
//! as an expression. A text scan would have to know about strings, comments and
//! backticks; the tree already does. Strings and comments are not `identifier`
//! nodes, and a backtick-escaped `` `fun` `` is an `identifier` whose text
//! includes the backticks, so it is not equal to `fun` and does not match.
//!
//! One legitimate `identifier` spelled like a keyword exists: `class` after `::`
//! in a class reference (`String::class`, `this::class`), which the grammar
//! reads as a `navigation_expression` of two identifiers. It is skipped by its
//! `::` sibling, measured on the locked grammar (the first draft of this
//! detector flagged every `X::class` in the corpus).
//!
//! The detector never looks inside an ERROR node: an error range already charges
//! the file there.
//!
//! # Bounded
//!
//! The walk checks the extraction deadline every [`DEADLINE_CHECK_STRIDE`] nodes
//! and stops at a node cap. Neither stop is read as "nothing lost": a deadline
//! passing fails the outcome exactly as `parse_outcome_of`'s own walk does, and a
//! node cap marks the whole file as an error range, because a check that could
//! not finish must never report what a check that ran and found nothing reports.

use tree_sitter::Node;

use crate::model::TextRange;
use crate::treesitter::{bounded_parent, walk_deadline_passed, DEADLINE_CHECK_STRIDE};

/// Words that cannot be an `identifier` in valid Kotlin and that open a
/// declaration.
const HARD_KEYWORDS: [&str; 7] = [
    "fun",
    "class",
    "interface",
    "object",
    "val",
    "var",
    "typealias",
];

/// The longest keyword, so a longer identifier is rejected without a compare.
const LONGEST_KEYWORD: usize = 9;

/// Node kinds an expression-shaped declaration is wrapped in, outermost last.
///
/// Climbed from the keyword to find the statement the grammar swallowed:
/// `annotated_expression` is one level per annotation, `infix_expression` holds
/// the modifier and keyword identifiers, and `assignment` wraps a declaration
/// with an initializer (`fun f() = 1`, `val y = 1`, `typealias T = Int`).
const WRAPPER_KINDS: [&str; 3] = ["annotated_expression", "assignment", "infix_expression"];

/// One statement the grammar read as an expression although it is a
/// declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LostRegion {
    /// The outermost wrapper node's byte range: the whole swallowed statement.
    pub(crate) range: TextRange,
    /// Start byte and spelling of every hard-keyword `identifier` inside the
    /// region, in document order. Recovery must account for each one.
    pub(crate) keywords: Vec<(usize, &'static str)>,
    /// Whether the region is a direct child of the file's root. Only these have
    /// the shape the recovery was measured on.
    pub(crate) top_level: bool,
    /// Whether the region's own subtree holds a genuine syntax error. Such a
    /// region is not a silent loss and is never recovered: its error range is
    /// the file's own.
    pub(crate) has_error: bool,
}

/// What the detector found.
#[derive(Debug, Default)]
pub(crate) struct LostScan {
    /// Sorted by start byte, one per distinct region.
    pub(crate) regions: Vec<LostRegion>,
    /// The node cap stopped the walk before it covered the tree.
    pub(crate) capped: bool,
    /// The extraction deadline passed during the walk.
    pub(crate) deadline_passed: bool,
}

impl LostScan {
    /// The ranges to charge to the file's parse outcome.
    pub(crate) fn ranges(&self) -> impl Iterator<Item = TextRange> + '_ {
        self.regions.iter().map(|region| region.range.clone())
    }
}

/// The node cap for a source of `len` bytes.
///
/// A tree has at most a couple of nodes per source byte (every token spans a
/// byte, and each named node wraps tokens), so this is never reached by a real
/// parse; it bounds a pathological one. The floor keeps tiny inputs from
/// tripping it on zero-width `MISSING` nodes.
fn node_cap(len: usize) -> usize {
    len.saturating_mul(4).saturating_add(4096)
}

/// Find every statement of a Kotlin tree that the grammar read as an
/// expression. Empty for a tree in which no hard keyword is an `identifier`.
pub(crate) fn detect(root: Node, source: &str) -> LostScan {
    detect_capped(root, source, node_cap(source.len()))
}

/// [`detect`] with an explicit node cap, so the cap can be exercised.
pub(crate) fn detect_capped(root: Node, source: &str, cap: usize) -> LostScan {
    let mut scan = LostScan::default();
    let bytes = source.as_bytes();
    let mut cursor = root.walk();
    let mut visited = 0usize;
    let mut since_check = 0u32;
    let mut found: Vec<LostRegion> = Vec::new();
    'walk: loop {
        let node = cursor.node();
        visited += 1;
        if visited > cap {
            scan.capped = true;
            break;
        }
        since_check += 1;
        if since_check >= DEADLINE_CHECK_STRIDE {
            since_check = 0;
            if walk_deadline_passed() {
                scan.deadline_passed = true;
                break;
            }
        }
        // An ERROR subtree is already charged by its own range.
        let descend = !node.is_error();
        if node.child_count() == 0 && node.kind() == "identifier" {
            let (start, end) = (node.start_byte(), node.end_byte());
            if end > start && end - start <= LONGEST_KEYWORD && !follows_double_colon(node) {
                let word = bytes.get(start..end).unwrap_or_default();
                if let Some(keyword) = HARD_KEYWORDS
                    .iter()
                    .find(|keyword| keyword.as_bytes() == word)
                {
                    record(node, start, keyword, &mut found);
                }
            }
        }
        if descend && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    found.sort_by_key(|region| (region.range.start_byte, region.range.end_byte));
    for region in found {
        match scan.regions.last_mut() {
            Some(last) if last.range == region.range => {
                last.keywords.extend(region.keywords);
            }
            _ => scan.regions.push(region),
        }
    }
    for region in &mut scan.regions {
        region.keywords.sort_unstable();
        region.keywords.dedup();
    }
    scan
}

/// Whether `node` is the member of a callable or class reference: the `class`
/// of `String::class`.
fn follows_double_colon(node: Node) -> bool {
    node.prev_sibling()
        .is_some_and(|before| before.kind() == "::")
}

/// File the region a keyword `identifier` sits in.
fn record(keyword: Node, start: usize, word: &'static str, found: &mut Vec<LostRegion>) {
    let mut top = keyword;
    while let Some(parent) = bounded_parent(top) {
        if WRAPPER_KINDS.contains(&parent.kind()) {
            top = parent;
        } else {
            break;
        }
    }
    let top_level = bounded_parent(top).is_some_and(|parent| parent.parent().is_none());
    found.push(LostRegion {
        range: TextRange {
            start_byte: top.start_byte(),
            end_byte: top.end_byte(),
        },
        keywords: vec![(start, word)],
        top_level,
        has_error: top.has_error(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_with(source: &str, cap: Option<usize>) -> LostScan {
        let (_, language) = crate::treesitter::grammar_for("kotlin").expect("kotlin grammar");
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).expect("load grammar");
        let tree = parser.parse(source, None).expect("tree");
        match cap {
            Some(cap) => detect_capped(tree.root_node(), source, cap),
            None => detect(tree.root_node(), source),
        }
    }

    fn regions(source: &str) -> Vec<&str> {
        scan_with(source, None)
            .regions
            .iter()
            .map(|region| &source[region.range.start_byte..region.range.end_byte])
            .collect()
    }

    /// The two annotations that make the grammar read a declaration as an
    /// expression.
    const STACK: &str = "@Preview\n@Composable\n";

    #[test]
    fn every_declaration_kind_behind_a_stack_is_a_region() {
        for declaration in [
            "private fun f() {}",
            "private fun f() = 1",
            "private class Q {}",
            "private object O {}",
            "private interface I {}",
            "internal class Q",
            "private val y = 1",
            "private var y = 1",
            "private typealias T = Int",
            "open class O",
            "sealed interface S",
            "enum class E { X }",
            "abstract class AC {}",
            "class Plain {}",
        ] {
            let source = format!("{STACK}{declaration}\nfun after() {{}}\n");
            let found = regions(&source);
            assert_eq!(
                found,
                vec![format!("{STACK}{declaration}").as_str()],
                "{declaration}"
            );
        }
    }

    #[test]
    fn a_region_is_top_level_only_at_the_root() {
        let top = scan_with(
            &format!("{STACK}private fun f() {{}}\nfun after() {{}}\n"),
            None,
        );
        assert_eq!(top.regions.len(), 1);
        assert!(top.regions[0].top_level);
        let nested = scan_with(
            &format!("fun g() {{\n{STACK}private fun h() {{}}\n}}\n"),
            None,
        );
        assert_eq!(nested.regions.len(), 1);
        assert!(!nested.regions[0].top_level);
    }

    #[test]
    fn a_region_lists_the_start_of_each_keyword_it_holds() {
        let source = format!("{STACK}private fun f() {{}}\nfun after() {{}}\n");
        let scan = scan_with(&source, None);
        assert_eq!(
            scan.regions[0].keywords,
            vec![(source.find("fun").unwrap(), "fun")]
        );
    }

    #[test]
    fn what_parses_as_a_declaration_is_not_a_region() {
        for source in [
            "private val X = 1\n@Preview\nprivate fun f() {}\nfun after() {}\n",
            "@Preview\n@Composable\nfun f() {}\n",
            "@Preview\n@Composable\nprivate fun f(): Int = 1\n",
            "@Preview\n@Composable\nprivate suspend fun s() {}\n",
            "@A\n@B\ndata class D(val a: Int)\n",
            "class K {\n    @A\n    @B\n    private fun m() {}\n}\n",
            "fun `fun`() {}\nval `class` = 1\n",
            "val s = \"private fun f() {}\"\n// @A @B private fun f() {}\n/* class X */\n",
            "val k = String::class.java\nfun <T : Any> name(c: kotlin.reflect.KClass<T>) = T::class\n",
            "interface Cb {\n    fun call()\n}\nval o = object {\n    val x = 1\n}\n",
        ] {
            assert!(regions(source).is_empty(), "{source}");
        }
    }

    #[test]
    fn a_keyword_inside_an_error_is_left_to_the_error_range() {
        // `class` as an expression after a stray `)`; the ERROR node is charged
        // by `parse_outcome_of`, so the detector adds nothing for it.
        let source = "fun f() {\n  ) class X {}\n}\n";
        let scan = scan_with(source, None);
        assert!(scan.regions.is_empty(), "{:?}", scan.regions);
    }

    #[test]
    fn a_walk_that_hits_the_node_cap_is_reported_not_clean() {
        let source = format!("{STACK}private fun f() {{}}\nfun after() {{}}\n");
        let scan = scan_with(&source, Some(3));
        assert!(scan.capped);
        assert!(
            scan.regions.is_empty(),
            "a capped walk found nothing, and says it is capped"
        );
    }
}
