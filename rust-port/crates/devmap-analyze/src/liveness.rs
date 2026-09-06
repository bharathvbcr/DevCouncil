use crate::model::*;
use devmap_extract::languages::{capabilities_for_language, Capability};
use devmap_extract::model::*;
use devmap_resolve::model::*;
use std::collections::{HashMap, HashSet};

/// Symbol identity relative to its file, so that `file_path` + `symbol_name`
/// reconstructs the graph id exactly. A method must report as `MyClass.execute`
/// rather than `execute`; the bare form cannot be joined back to a node and
/// collides with any same-named method on a different type in the same file.
fn dead_symbol_identity(symbol: &ExtractedSymbol, file_path: &str) -> String {
    symbol
        .qualified_name
        .strip_prefix(file_path)
        .and_then(|rest| rest.strip_prefix("::"))
        .map(str::to_string)
        .unwrap_or_else(|| symbol.name.clone())
}

/// Directory owning a path, used as half of a Go package identity. Two packages
/// with the same name in different directories are unrelated, so the name alone
/// cannot key the interface join.
fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

/// Go package identity of an extraction: `(directory, package clause)`.
fn go_package_key(ext: &Extraction) -> Option<(&str, &str)> {
    if ext.language != "go" {
        return None;
    }
    let package = ext.go_package.as_deref().filter(|name| !name.is_empty())?;
    Some((parent_dir(&ext.file_path), package))
}

/// Interface method specs grouped by declaring Go package.
///
/// Scoping to the package is exact, not merely conservative: an *unexported*
/// interface method name is qualified by the package that declared it, so no
/// type outside that package can ever satisfy it. Exported methods can be
/// satisfied cross-package, but an exported Go method already reports
/// `is_exported`, so it never needs this exemption. Widening to the whole corpus
/// would therefore buy nothing and would exempt every same-named method in every
/// unrelated package.
fn go_interface_specs_by_package(
    extractions: &[Extraction],
) -> HashMap<(&str, &str), Vec<&GoInterfaceMethod>> {
    let mut by_package: HashMap<(&str, &str), Vec<&GoInterfaceMethod>> = HashMap::new();
    for ext in extractions {
        let Some(key) = go_package_key(ext) else {
            continue;
        };
        if ext.go_interface_methods.is_empty() {
            continue;
        }
        by_package
            .entry(key)
            .or_default()
            .extend(ext.go_interface_methods.iter());
    }
    by_package
}

/// Grammar keys of the C family, as `Extraction::language` reports them. Metal
/// answers `cpp`, because it borrows that grammar.
fn is_c_family_language(language: &str) -> bool {
    matches!(language, "c" | "cpp" | "objc" | "cuda")
}

/// Whether `path` is a C-family header. Mirrors `is_c_header_path` in the
/// extractor, which decides both `is_exported` and which prototypes become
/// exports. The two must agree on every extension or the join silently half
/// fires: a file the extractor calls a header but this does not would publish
/// exports while its own symbols were still treated as private. The behavioural
/// consequence is pinned by `the_two_header_tables_agree_extension_by_extension`.
fn is_c_header_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    C_HEADER_EXTENSIONS
        .iter()
        .any(|extension| lower.ends_with(extension))
}

/// The C-family header extensions, taken from the frozen `LANGUAGE_SPECS`
/// rather than from what C projects can in principle be spelled with.
///
/// `.h` belongs to C, `.hh`/`.hpp`/`.hxx` to C++, `.cuh` to CUDA. Extensions the
/// registry does not list — `.h++`, `.inl`, `.tcc` — never reach a C-family
/// grammar at all, so listing them here would be configuration that can never
/// fire. An earlier draft did list them and the agreement test caught it.
const C_HEADER_EXTENSIONS: &[&str] = &[".h", ".hh", ".hpp", ".hxx", ".cuh"];

/// Every name published by a C-family header anywhere in the corpus.
///
/// The cross-file half of C-family visibility. A definition in a `.c`/`.cpp` is
/// not itself an export — the header publishes it — so a definition whose name a
/// header declares is public API and must never be a confident dead-code
/// candidate. Extraction cannot answer this because a header and its
/// implementation are different files; this is the same split SC6a used for Go
/// interface methods.
///
/// Measured on a 183-file first-party corpus, this is not a hypothetical: of 118
/// confident findings without it, 37 were libyaml's entire public API
/// (`yaml_emitter_delete`, `yaml_parser_set_input`, …) declared in `yaml.h` and
/// called only by the Swift package that wraps it, and a further block were
/// cgo's `x_cgo_*` entry points declared in `libcgo.h` and called from Go
/// assembly. Every one would have been a proposal to delete working code.
///
/// Scoped corpus-wide rather than per-directory because a C header can be
/// included from anywhere, unlike a Go package. The cost is that a header
/// declaring a very common name exempts same-named definitions elsewhere; that
/// errs toward missing a finding rather than toward proposing a deletion that
/// breaks a build, which is the direction SC6a settled on for the same trade.
fn c_header_exported_names(extractions: &[Extraction]) -> HashSet<&str> {
    let mut names = HashSet::new();
    for ext in extractions {
        if !is_c_family_language(&ext.language) || !is_c_header_path(&ext.file_path) {
            continue;
        }
        for export in &ext.exports {
            if !export.exported_name.is_empty() {
                names.insert(export.exported_name.as_str());
            }
        }
    }
    names
}

/// Why a build-variant finding is exempt rather than merely downgraded.
///
/// Named once so the analyzer and the tests that pin this behaviour cannot
/// drift into describing the same exemption two different ways.
pub const GO_BUILD_VARIANT_REASON: &str =
    "Go build-constrained variant — the call reaches whichever variant this build selects";

/// Symbol identities that exist in a Go package only as mutually exclusive
/// build variants, keyed by `(package, identity)`.
///
/// Go forbids two package-level declarations of one name. A package that
/// declares `configureProcessGroup` in both `procgroup_unix.go` and
/// `procgroup_other.go` therefore cannot compile unless those files are
/// mutually exclusive — and they are, by `//go:build unix` and `//go:build
/// !unix`. Exactly one reaches any given build, so a call naming that identity
/// reaches whichever one compiled. All of them are live.
///
/// The resolver cannot see this: it finds N definitions of one name, cannot
/// pick between them, and emits `AmbiguousGlobal`. Liveness then downgrades
/// every candidate to `only_ambiguous_callers` — which reads as "this might be
/// dead" about code that is guaranteed to be running. Measured on a Go-heavy
/// external corpus, this was **all 16** of its non-exempt findings.
///
/// The join is sound rather than merely convenient because of Go's own
/// visibility rule, which the resolver already enforces in
/// `go_symbol_visible_from`: an *unexported* name resolves only within its own
/// directory, and an exported one reports `is_exported` and never reaches this
/// branch at all. So the ambiguity behind one of these findings is necessarily
/// within a single package, which is precisely where the uniqueness rule bites.
///
/// Requiring **every** declaring file to carry a constraint is the part that
/// keeps this honest. Two unconstrained files declaring one name is not a build
/// variant — it is a package that does not compile, or an extraction bug, and
/// either way it is not evidence that the symbol is alive.
fn go_build_variant_identities(extractions: &[Extraction]) -> HashSet<(&str, &str, String)> {
    // (package key, identity) -> (files seen, files carrying a constraint)
    let mut seen: HashMap<(&str, &str, String), (usize, usize)> = HashMap::new();
    for ext in extractions {
        let Some((dir, package)) = go_package_key(ext) else {
            continue;
        };
        // One file declaring a name twice is not two files declaring it, and Go
        // would reject it anyway; count each file at most once per identity.
        let mut in_this_file: HashSet<String> = HashSet::new();
        for sym in &ext.symbols {
            if sym.kind == SymbolKind::File {
                continue;
            }
            let identity = dead_symbol_identity(sym, &ext.file_path);
            if !in_this_file.insert(identity.clone()) {
                continue;
            }
            let entry = seen.entry((dir, package, identity)).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += usize::from(ext.go_build_constrained);
        }
    }
    seen.into_iter()
        .filter(|(_, (files, constrained))| *files >= 2 && files == constrained)
        .map(|(key, _)| key)
        .collect()
}

/// How much of the corpus the extraction tier could not read.
///
/// The cross-file half of X6. `analyze_liveness` already refuses to call a
/// parse-failed file's *own* symbols dead, because "nothing calls it" is only
/// evidence when calls were looked for. The same sentence is true one hop out:
/// a file that contributed no call edges was also the only possible caller of
/// somebody else's symbol, and nothing downstream knew that the reachability
/// scan had a hole in it.
///
/// Kept as two numbers rather than one because they are different claims. A
/// `Failed` file contributed nothing at all; a `Fallback` file contributed
/// names and spans but, by construction, no calls and no imports. Both lose
/// edges, and a reader deciding whether to act on a dead-code finding wants to
/// know which kind of blindness they are looking at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractionCoverage {
    /// Files a grammar was wanted for and did not get to read.
    ///
    /// Counted through [`Extraction::is_parse_failure`], the canonical owner,
    /// **not** through a bare `matches!(parse_outcome, Failed { .. })`. Prose
    /// and data formats report `Failed` for want of a grammar that does not
    /// exist and never will; on this repository that is 294 of 1,310 files, all
    /// Markdown, JSON, YAML, config and HTML. Counting those would report every
    /// build of every real repository as degraded, and a degraded flag that is
    /// always on carries no information at all.
    pub parse_failed_files: usize,
    /// Files whose declarations were recovered by line pattern.
    ///
    /// A `.proto` or `.ps1` this build cannot parse is a genuine gap in call
    /// coverage — see [`ExtractionEngine::NotApplicable`]'s own docs drawing
    /// exactly this line against a `.md`.
    pub pattern_recovered_files: usize,
    /// Files discovery refused before any extractor saw them — oversized,
    /// unreadable, or a non-UTF-8 path.
    ///
    /// **Not derivable from `extractions`**, which is precisely why this gap
    /// outlived the parse-failure one it otherwise resembles: a refused file has
    /// no `Extraction` at all, so every coverage check computed from that slice
    /// reported a complete corpus. The count has to be carried in from
    /// discovery, and the two production build paths do that.
    ///
    /// The cost of missing it, measured: `lib.py` defines `helper()`, its only
    /// caller `app.py` is over `MAX_SOURCE_BYTES`, and `devmap dead` proposed
    /// deleting `helper` at 0.9 — the confident tier — because the file that
    /// calls it was never read.
    pub discovery_refused_files: usize,
    /// Files a grammar read cleanly whose language has no call extractor.
    ///
    /// The gap the other three could not represent, because it is not a
    /// failure of any kind: the parse succeeded. `CALL_EXTRACTION_LANGUAGES`
    /// was supposed to be read by "the coverage report" so this would be a
    /// stated fact — no such reader existed, and a CFML or Terraform file
    /// therefore reported full coverage while contributing not one call edge.
    pub call_blind_files: usize,
    /// Files a grammar read cleanly whose language has no import extractor.
    ///
    /// Counted, published, and deliberately kept out of `is_complete()` — see
    /// the note there. Its consumer is `unwired_candidates`, whose entire
    /// question is "does an inbound `Imports` edge exist", and which for 24 of
    /// 35 languages was answering it from an absence the extractor created.
    pub import_blind_files: usize,
}

/// What discovery refused, for the analysis that cannot see it.
///
/// A separate type rather than a bare `usize` so a caller cannot pass the wrong
/// count positionally, and so the one place that means "no discovery step ran"
/// is spelled [`DiscoveryCoverage::none`] rather than `0`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscoveryCoverage {
    /// `None` means no discovery result is being reported — which is **not**
    /// the same as a discovery step that ran and refused nothing. Keeping the
    /// two apart is the whole reason this is an `Option` and not a `usize`: a
    /// summary that records `0` for the first case tells a later reader the
    /// tree was fully walked when nobody walked it.
    refused_files: Option<usize>,
}

impl DiscoveryCoverage {
    /// No discovery step ran, so there is no refusal count to report.
    ///
    /// Correct for a caller that supplies its own corpus directly — a test, or
    /// the single-file preview path. Wrong for anything that walked a tree, and
    /// that is the distinction this exists to keep visible.
    pub fn none() -> Self {
        Self {
            refused_files: None,
        }
    }

    /// A discovery step ran and refused this many files. `refused(0)` is a
    /// measurement, and says more than [`DiscoveryCoverage::none`] does.
    pub fn refused(refused_files: usize) -> Self {
        Self {
            refused_files: Some(refused_files),
        }
    }

    /// The measurement, or `None` when none was taken. Persisted verbatim so a
    /// later generation can tell "measured, nothing refused" from "never
    /// measured" instead of rounding both to zero.
    pub fn refused_files(&self) -> Option<usize> {
        self.refused_files
    }

    /// What to charge against coverage.
    ///
    /// An unmeasured discovery contributes nothing, deliberately. Charging it
    /// would put every caller that builds its own corpus — every test, and the
    /// single-file preview path — permanently in a degraded state, and a marker
    /// that is always on is worth exactly as much as one that is never on.
    pub fn charged(&self) -> usize {
        self.refused_files.unwrap_or(0)
    }
}

impl ExtractionCoverage {
    /// Whether every file in the corpus had its calls looked for.
    ///
    /// `import_blind_files` is deliberately **not** here. It is a hole in a
    /// different claim: no `Imports` edge is evidence about
    /// `unwired_candidates`, which is where W0.3 charges it, and folding it in
    /// would cap every dead-code finding in every Java, C++, Ruby, Swift, C#
    /// and PHP repository at `ambiguous` — that is most of the world's code,
    /// demoted for a blindness that is not the one the verdict rests on. The
    /// existing two counters are kept apart for exactly this reason ("different
    /// claims"), and a third that means something else again gets the same
    /// treatment.
    pub fn is_complete(&self) -> bool {
        self.parse_failed_files == 0
            && self.pattern_recovered_files == 0
            && self.discovery_refused_files == 0
            && self.call_blind_files == 0
    }

    /// Files that contributed no call edges, of any kind.
    pub fn files_without_call_extraction(&self) -> usize {
        self.parse_failed_files + self.pattern_recovered_files + self.call_blind_files
    }

    /// Why the corpus-level scan is incomplete, or `None` when it is complete.
    ///
    /// `None` on a clean corpus is the whole point: this string is what
    /// `analyze()` folds into `AnalysisStatus::Partial`, which drives
    /// `graph_degraded` and `analysis_status`. A repository whose every file
    /// parsed must keep reporting `ok`.
    pub fn degraded_reason(&self) -> Option<String> {
        if self.is_complete() {
            return None;
        }
        let mut reason = format!(
            "call extraction did not cover the whole corpus: {} file(s) failed to parse, \
             {} recovered by pattern (no calls extracted), {} refused by discovery and never \
             read at all — dead-code and unwired findings are a lower bound and are capped \
             below the confident tier",
            self.parse_failed_files, self.pattern_recovered_files, self.discovery_refused_files
        );
        // Appended rather than folded into the sentence above, because it is a
        // different kind of fact and a permanent one. The other three describe
        // this run — a file that happened to fail, a walk that happened to
        // refuse. This one describes the build: no amount of re-running
        // extracts a call from a `.cfm`, and a reader deciding whether to
        // re-index needs to know which of the two they are looking at.
        if self.call_blind_files > 0 {
            reason.push_str(&format!(
                "; {} file(s) in a language with no call extractor at all \
                 (permanent for this build, not a transient failure)",
                self.call_blind_files
            ));
        }
        Some(reason)
    }

    /// Ceiling applied to a non-exempt dead-code confidence while the scan has
    /// a hole in it, leaving anything below it untouched.
    fn cap(&self, confidence: f32) -> f32 {
        if self.is_complete() {
            confidence
        } else {
            confidence.min(COVERAGE_LOSS_CONFIDENCE_CAP)
        }
    }
}

/// Confidence ceiling for a dead-code finding made against a partially read
/// corpus.
///
/// Sits below `INFERRED_FLOOR_MILLIS` (400) so `code_graph.rs::confidence_label`
/// renders `ambiguous` rather than `inferred` or `extracted`, and above the
/// 0.3 the exempt tier uses so the two stay distinguishable. `extracted` is the
/// tier `CLAUDE.md` tells agents to act on; a check that could not run must
/// never reach it.
pub const COVERAGE_LOSS_CONFIDENCE_CAP: f32 = 0.35;

/// Reason carried by a confident finding that the coverage cap demoted.
///
/// The unqualified branch previously carried `None`, which `code_graph.rs`
/// renders as "no inbound call edges and not exported" — a claim the run was
/// not entitled to make. `only_ambiguous_callers` is deliberately left alone:
/// it is a machine token three tests match exactly, and its own tier already
/// reads as unconfirmed.
pub const COVERAGE_LOSS_REASON: &str =
    "no inbound call edges, but call extraction did not cover every file — not evidence of death";

/// Which kind of hole one file leaves in call coverage.
///
/// The two are not interchangeable and are never folded into one number: a
/// `ParseFailed` file contributed nothing at all, a `PatternRecovered` one
/// contributed names and spans but, by construction, no calls and no imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionGap {
    ParseFailed,
    PatternRecovered,
    /// A grammar read the file cleanly and this build has no call extractor for
    /// its language.
    ///
    /// The hole the other two could not see. A `.cfm` or a `.tf` parses
    /// `Clean`, so `is_parse_failure` is false and `Fallback` never matches —
    /// the file sailed past both gaps, `is_complete()` stayed true, and every
    /// top-level symbol in it was published at the `extracted` tier, the one
    /// `CLAUDE.md` tells agents is safe to act on. "Nothing calls it" was a
    /// statement about the extractor and read as a statement about the code.
    CallBlind,
    /// A grammar read the file cleanly and this build extracts no imports for
    /// its language.
    ///
    /// Charged separately from [`Self::CallBlind`] and **not** folded into
    /// `is_complete()`: it undermines `unwired_candidates`, not the dead-symbol
    /// verdict. See `ExtractionCoverage::is_complete`.
    ImportBlind,
}

impl ExtractionGap {
    /// The stored spelling, and the one a consumer reads back. One owner, so a
    /// persisted inventory and an in-memory count cannot disagree about what a
    /// gap is called.
    pub fn label(self) -> &'static str {
        match self {
            ExtractionGap::ParseFailed => "parse_failed",
            ExtractionGap::PatternRecovered => "pattern_recovered",
            ExtractionGap::CallBlind => "call_blind",
            ExtractionGap::ImportBlind => "import_blind",
        }
    }
}

/// How far the override join walks a heritage chain.
///
/// `Derived -> Middle -> Base` is two hops and ordinary; a bound past this is
/// either generated code or a cycle, and a cycle in a heritage graph is not
/// expressible in any language here but is expressible in a *graph*, which is
/// what this walks. Bounded rather than trusted.
const HERITAGE_WALK_MAX_DEPTH: usize = 8;

/// Reason carried by an override the base type's call reaches.
///
/// Names the supertype, because the exemption is only as good as the edge
/// behind it and a reader deciding whether to trust it needs to see which
/// relation fired.
fn heritage_override_reason(supertype: &str) -> String {
    format!(
        "Overrides a method reached through `{supertype}` — polymorphic dispatch, \
         matched by name on a resolved heritage edge"
    )
}

/// `(file, TypeName)` -> the supertypes it declares, from `Extends`/`Implements`.
///
/// These edges exist at all only since W1.2: `ReferenceKind::Heritage` and both
/// edge kinds were declared with no producer, so a method reached only through
/// its base type had no inbound edge and every override was a candidate
/// `extracted` false positive.
///
/// Ambiguous edges are excluded for the same reason the call join excludes
/// them: an unresolved supertype is evidence that a base *might* exist, not
/// proof of which one, and a speculative edge must not exempt a symbol.
fn supertypes_by_type(resolution: &ResolutionResult) -> HashMap<(&str, &str), Vec<(&str, &str)>> {
    let mut by_type: HashMap<(&str, &str), Vec<(&str, &str)>> = HashMap::new();
    for edge in &resolution.edges {
        if !matches!(edge.edge_kind, EdgeKind::Extends | EdgeKind::Implements) {
            continue;
        }
        if matches!(
            edge.resolution.as_deref(),
            Some(Resolution::AmbiguousGlobal { .. }) | Some(Resolution::Unresolved { .. })
        ) {
            continue;
        }
        let Some(declarer) = edge.source_symbol.rsplit("::").next() else {
            continue;
        };
        let Some(supertype) = edge.target_symbol.rsplit("::").next() else {
            continue;
        };
        by_type
            .entry((edge.source_file.as_str(), declarer))
            .or_default()
            .push((edge.target_file.as_str(), supertype));
    }
    by_type
}

/// Whether a call to a supertype's same-named method reaches this override.
///
/// The generalisation of the Go interface pre-pass, which matches on name plus
/// arity within one package and produces a wiring exemption. This one runs on a
/// real resolved edge, so it works across files and across languages, and it
/// still matches the *method* by name only — a supertype's `render` and an
/// override's `render` are joined because they are spelled the same, which is
/// what an override is.
fn reached_through_a_supertype(
    file: &str,
    identity: &str,
    supertypes: &HashMap<(&str, &str), Vec<(&str, &str)>>,
    called: &HashSet<(String, String)>,
) -> Option<String> {
    // Only a method can be an override: `Type.method` is the identity shape
    // `dead_symbol_identity` produces, and a bare name is a free function.
    let (declaring_type, method) = identity.rsplit_once('.')?;

    let mut frontier = vec![(file, declaring_type)];
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for _ in 0..HERITAGE_WALK_MAX_DEPTH {
        let mut next = Vec::new();
        for key in frontier {
            if !seen.insert(key) {
                continue;
            }
            for (super_file, super_name) in supertypes.get(&key).into_iter().flatten() {
                // Either spelling of the call target: the qualified
                // `Base.render` an edge names, or the bare `render` the short
                // name is also recorded under.
                let qualified = format!("{super_name}.{method}");
                if called.contains(&(super_file.to_string(), qualified))
                    || called.contains(&(super_file.to_string(), method.to_string()))
                {
                    return Some(heritage_override_reason(super_name));
                }
                next.push((*super_file, *super_name));
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    None
}

/// Reason carried by a finding an unresolved call site vetoed.
///
/// States the imprecision in the reason itself rather than in a code comment,
/// because the reason is what a reader acts on. `UnresolvedReference` carries
/// no `target_file`, so the join is name-only and corpus-wide — the same trade
/// `c_header_exported_names` already makes for C headers, and for the same
/// reason: the alternative is a confident verdict resting on a resolver's
/// failure.
pub const UNRESOLVED_NAMESAKE_REASON: &str =
    "an unresolved call site names this symbol — the resolver could not bind that site to \
     anything, so \"nothing calls this\" is a statement about the resolver, not the code \
     (matched by name across the whole corpus; the ledger records no target file)";

/// Names that some call site meant and the resolver could not bind.
///
/// The kernel keeps a six-tier ledger of every site the resolution ladder gave
/// up on, and the dead-code pass never read it. If an unresolved site names
/// `foo`, then "nothing calls `foo`" describes the resolver rather than the
/// code, and publishing it at the `extracted` tier — whose contract is "safe to
/// act on" — is the unattributed tier silently manufacturing confident
/// findings.
///
/// **Only two classes qualify, not the five that are not `Builtin`.** Every
/// other class carries affirmative evidence that the site meant something else,
/// and admitting it would veto real findings on a coincidence of spelling:
///
/// * `Builtin` — the name is declared by the language (`len`, `print`). A
///   closed set; no indexed file can declare these.
/// * `HostGlobal` — declared by the runtime (`setTimeout`, `fetch`), with the
///   authority named in the row.
/// * `LocalBinding` — the *enclosing symbol itself* declares the name, as a
///   parameter or a local closure. Its own documentation calls the ladder
///   failing here "the correct outcome rather than a defect". Admitting it
///   would mean any function with a local named `render` resurrects every dead
///   `render` in the corpus.
/// * `External { module }` — bound by an import whose specifier names no
///   indexed file. The import statement is the evidence. A *repo-relative*
///   specifier is filed under `Unresolved` instead, precisely so this class
///   stays import-proven.
///
/// What is left is exactly the two tiers where the resolver admits it does not
/// know: `UninferredReceiver` (the receiver exists and could not be typed) and
/// `Unresolved` (a bare name nothing explains — "the only tier that indicates a
/// defect").
///
/// `Route` joins `Call` and `Reference` as an admitted kind because a route
/// handler that failed to bind is the strongest version of this case: the
/// `HandlesRoute` edge is what tells liveness a handler is reached from outside
/// the call graph at all, so an unbound one leaves a live handler looking dead.
/// `Import` is excluded because its `callee_name` is a *module specifier*, not
/// a symbol name, and matching specifiers against symbols is noise.
fn unresolved_namesakes(resolution: &ResolutionResult) -> HashSet<&str> {
    resolution
        .unresolved
        .iter()
        .filter(|row| {
            matches!(
                row.kind,
                UnresolvedKind::Call | UnresolvedKind::Reference | UnresolvedKind::Route
            ) && matches!(
                row.class,
                UnresolvedClass::UninferredReceiver | UnresolvedClass::Unresolved
            )
        })
        .map(|row| row.callee_name.as_str())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Reason carried by a finding the call-blind cap demoted.
///
/// Distinct from [`COVERAGE_LOSS_REASON`] on purpose. That one says extraction
/// "did not cover every file", which invites the reader to re-index. For a
/// language with no extractor there is nothing to re-run: the fact is about
/// this build's capabilities, it is permanent until someone writes the
/// extractor, and saying so is the difference between a transient gap and a
/// structural one.
pub const CALL_BLIND_REASON: &str =
    "no inbound call edges, but this file's language has no call extractor in this build — \
     the absence is the extractor's, not the code's";

/// Whether a grammar actually read this file.
///
/// The gate that keeps call-blindness from swallowing the tree. Prose and data
/// formats report `NotApplicable` and declare no capabilities, so without this
/// every `.md`, `.json` and `.yaml` would be charged as call-blind — 294 of
/// this repository's 1,310 files — and `is_complete()` would be false on every
/// corpus in existence. A degraded flag that is always on carries no
/// information, which is the same trap `ExtractionCoverage::parse_failed_files`
/// documents for its own count.
///
/// `RegexFallback` and `Unavailable` are excluded for a different reason: they
/// are already charged, as `PatternRecovered` and `ParseFailed` respectively.
/// Charging them again would double-count one file's single hole.
fn a_grammar_read_this_file(ext: &Extraction) -> bool {
    matches!(
        ext.engine,
        ExtractionEngine::TreeSitter { .. } | ExtractionEngine::Notebook { .. }
    ) && matches!(
        ext.parse_outcome,
        ParseOutcome::Clean | ParseOutcome::Partial { .. }
    )
}

/// One file that call extraction did not cover, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionGapEntry {
    pub path: String,
    pub gap: ExtractionGap,
    pub reason: String,
}

/// Name the files whose calls were never extracted.
///
/// The owner of the *set*; [`extraction_coverage`] is the fold over it, so a
/// count and a list of paths cannot disagree about which files they describe.
/// That mattered as soon as `devmap status` began naming them: a count derived
/// from one `matches!` chain and a list derived from another is precisely how
/// "2 file(s) failed to parse" came to sit beside a list of three.
pub fn extraction_gaps(extractions: &[Extraction]) -> Vec<ExtractionGapEntry> {
    let mut gaps = Vec::new();
    for ext in extractions {
        // `Extraction::is_parse_failure` is the canonical owner of the
        // `Failed`-vs-`NotApplicable` line — a `.md` is not a parse failure —
        // and asking it here is what keeps this list and the counts below in
        // step with it.
        let (gap, reason) = if ext.is_parse_failure() {
            (
                ExtractionGap::ParseFailed,
                match &ext.parse_outcome {
                    ParseOutcome::Failed { reason } => reason.clone(),
                    // Unreachable while `is_parse_failure` matches `Failed`,
                    // and stated rather than `unwrap`ped: a later variant that
                    // qualifies must still name itself in the inventory.
                    other => format!("{other:?}"),
                },
            )
        } else if let ParseOutcome::Fallback { reason } = &ext.parse_outcome {
            (ExtractionGap::PatternRecovered, reason.clone())
        } else if a_grammar_read_this_file(ext) {
            // A clean parse in a language this build has no extractor for.
            // Both bits are asked independently: HCL is call-blind *and*
            // import-blind, Java only the second, and collapsing them would
            // make a file with one hole indistinguishable from a file with two.
            let capabilities = capabilities_for_language(&ext.language);
            if !capabilities.contains(Capability::Calls) {
                gaps.push(ExtractionGapEntry {
                    path: ext.file_path.clone(),
                    gap: ExtractionGap::CallBlind,
                    reason: format!("`{}` has no call extractor in this build", ext.language),
                });
            }
            if !capabilities.contains(Capability::Imports) {
                gaps.push(ExtractionGapEntry {
                    path: ext.file_path.clone(),
                    gap: ExtractionGap::ImportBlind,
                    reason: format!("`{}` has no import extractor in this build", ext.language),
                });
            }
            continue;
        } else {
            continue;
        };
        gaps.push(ExtractionGapEntry {
            path: ext.file_path.clone(),
            gap,
            reason,
        });
    }
    gaps
}

/// Count the files whose calls were never extracted.
///
/// One owner for the question, shared with `code_graph.rs`, which needs the
/// same two numbers for `meta.devmap_rust`. Two independent `matches!` chains
/// over `parse_outcome` is exactly how the `Failed`-vs-`NotApplicable`
/// distinction gets lost in one of them — so this counts
/// [`extraction_gaps`]'s entries rather than re-deciding them.
pub fn extraction_coverage(extractions: &[Extraction]) -> ExtractionCoverage {
    let mut coverage = ExtractionCoverage::default();
    for entry in extraction_gaps(extractions) {
        match entry.gap {
            ExtractionGap::ParseFailed => coverage.parse_failed_files += 1,
            ExtractionGap::PatternRecovered => coverage.pattern_recovered_files += 1,
            ExtractionGap::CallBlind => coverage.call_blind_files += 1,
            ExtractionGap::ImportBlind => coverage.import_blind_files += 1,
        }
    }
    coverage
}

/// Dead-symbol findings together with how much of the corpus produced them.
pub struct LivenessOutcome {
    pub reports: Vec<DeadSymbolReport>,
    pub coverage: ExtractionCoverage,
}

/// Dead-symbol findings only.
///
/// Thin delegate over [`analyze_liveness_with_coverage`], kept because callers
/// that only want the findings should not have to name the coverage record.
/// The confidence cap is applied by the canonical implementation, so both entry
/// points report the same tiers.
pub fn analyze_liveness(
    extractions: &[Extraction],
    resolution: &ResolutionResult,
) -> Vec<DeadSymbolReport> {
    analyze_liveness_with_coverage(extractions, resolution, DiscoveryCoverage::none()).reports
}

pub fn analyze_liveness_with_coverage(
    extractions: &[Extraction],
    resolution: &ResolutionResult,
    discovery: DiscoveryCoverage,
) -> LivenessOutcome {
    let mut coverage = extraction_coverage(extractions);
    // Folded in before the cap is applied, not after the reports are built: a
    // file discovery never read may hold the only call to a symbol here, so a
    // refusal has to reach `coverage.cap()` the same way a parse failure does.
    coverage.discovery_refused_files = discovery.charged();
    // Computed once for the whole corpus: the join is name-only, so it has no
    // per-file component to recompute.
    let unresolved_names = unresolved_namesakes(resolution);
    let supertypes = supertypes_by_type(resolution);
    let go_interface_specs = go_interface_specs_by_package(extractions);
    let c_header_exports = c_header_exported_names(extractions);
    let go_build_variants = go_build_variant_identities(extractions);

    // File-scoped called symbols: (target_file, symbol_name_or_qualified_name)
    let mut called_symbols: HashSet<(String, String)> = HashSet::new();
    let mut ambiguous_symbols: HashSet<(String, String)> = HashSet::new();

    for edge in &resolution.edges {
        // Structural edges describe where a symbol *lives*, not that anything
        // uses it. A file containing a symbol, or a type owning its method, is
        // not a call: counting it would mark every declared symbol as reached
        // and silently disable dead-code detection entirely.
        if matches!(
            edge.edge_kind,
            EdgeKind::Contains | EdgeKind::Defines | EdgeKind::MemberOf
        ) {
            continue;
        }
        // An ambiguous or explicitly unresolved edge is evidence that a
        // symbol may be called, not proof that any one candidate is called.
        // Do not turn speculative resolution into a false liveness negative.
        if matches!(
            edge.resolution.as_deref(),
            Some(Resolution::AmbiguousGlobal { .. })
        ) {
            ambiguous_symbols.insert((edge.target_file.clone(), edge.target_symbol.clone()));
            if let Some(short_name) = edge.target_symbol.rsplit("::").next() {
                ambiguous_symbols.insert((edge.target_file.clone(), short_name.to_string()));
                // …and the member name with its owner stripped.
                //
                // `ExtractedSymbol::name` is the bare `toJson`, while an edge
                // names `File::RemoteTaskEntity.toJson`, so without this the two
                // never meet and the ambiguity is recorded against nothing.
                //
                // This became load-bearing when Kotlin extension functions
                // gained their receiver: one Android file declares seven
                // `private fun <T>.toJson()` on seven different types and calls
                // every one of them as `it.toJson()` inside a `map { }`. The
                // receiver `it` cannot be typed, so the resolver emits an
                // *ambiguous* edge naming one candidate — and the other six,
                // each genuinely called, were reported dead at 0.9 confidence.
                // Before the receiver fix all seven collapsed into one symbol
                // and the question could not arise.
                //
                // Bounded to the ambiguous set on purpose: this can only move a
                // finding from 0.9 to 0.4 `only_ambiguous_callers`, never exempt
                // it, so it cannot hide a symbol nothing calls. Doing the same
                // for `called_symbols` would silently exempt every same-named
                // method in the file, which is a different and much worse trade.
                if let Some(member) = short_name.rsplit('.').next() {
                    ambiguous_symbols.insert((edge.target_file.clone(), member.to_string()));
                }
            }
            continue;
        }
        if matches!(
            edge.resolution.as_deref(),
            Some(Resolution::Unresolved { .. })
        ) {
            continue;
        }
        called_symbols.insert((edge.target_file.clone(), edge.target_symbol.clone()));
        if let Some(short_name) = edge.target_symbol.rsplit("::").next() {
            called_symbols.insert((edge.target_file.clone(), short_name.to_string()));
        }
    }

    let mut reports = Vec::new();

    for ext in extractions {
        // X6: Parse-failed files must NEVER be reported as confirmed dead code
        // candidates — and neither must pattern-recovered ones.
        //
        // A `Fallback` file had its declarations recovered by line pattern
        // because no grammar exists for its language, and that tier extracts no
        // calls at all. So *every* symbol in such a file is uncalled by
        // construction, and reporting them would hand `devmap dead` one false
        // candidate per declaration in every `.proto`, `.ps1` and `.vb` in the
        // tree. "Nothing calls it" is only evidence when calls were looked for.
        let is_parse_failed = matches!(
            ext.parse_outcome,
            ParseOutcome::Failed { .. } | ParseOutcome::Fallback { .. }
        );

        // The same sentence as `is_parse_failed`, one step further out: a file
        // whose grammar succeeded but whose language has no call extractor also
        // extracted no calls, so every symbol in it is uncalled by
        // construction. `is_parse_failed` could not see this because the parse
        // did not fail — that is exactly how CFML and Terraform symbols reached
        // the `extracted` tier.
        let file_is_call_blind = a_grammar_read_this_file(ext)
            && !capabilities_for_language(&ext.language).contains(Capability::Calls);

        // A wiring annotation is file-scoped only when it targets the file
        // itself. Symbol-scoped annotations must never be read as file-scoped:
        // one `#[test] fn` would otherwise exempt every symbol in the file,
        // which is the same over-exemption the file-level decorator rule
        // already suffers from.
        let (file_wiring, symbol_wiring): (Vec<_>, Vec<_>) = ext
            .wiring
            .iter()
            .partition(|w| w.target_symbol == ext.file_path);

        let is_file_exempt = is_parse_failed
            || file_wiring.iter().any(|w| {
                matches!(
                    w.kind,
                    WiringKind::Vendored
                        | WiringKind::TestFile
                        | WiringKind::GeneratedFile
                        | WiringKind::ScriptEntry
                        | WiringKind::StructuralExempt
                        | WiringKind::FrameworkDecorator
                        | WiringKind::Launcher
                        | WiringKind::ReExportPackage
                )
            });

        // Per-symbol exemptions: a runtime, framework, or harness reaches the
        // symbol without an explicit call site, or the language forbids the
        // symbol from ever being marked public. Keyed by `qualified_name`,
        // which is what the extractor writes into `target_symbol`.
        let symbol_exemptions: HashMap<&str, &str> = symbol_wiring
            .iter()
            .filter(|w| {
                matches!(
                    w.kind,
                    WiringKind::RuntimeEntryPoint | WiringKind::StructuralExempt
                )
            })
            .map(|w| (w.target_symbol.as_str(), w.details.as_str()))
            .collect();

        // Cross-file half of the Go interface exemption. Extraction closes the
        // same-file case as a wiring annotation; the package-wide join can only
        // happen here, where every file is in scope.
        let go_interface_exemptions: HashMap<&str, String> = go_package_key(ext)
            .and_then(|key| go_interface_specs.get(&key))
            .map(|specs| {
                let param_counts = ext.go_method_param_counts();
                go_interface_method_matches(&ext.symbols, &param_counts, specs.iter().copied())
                    .into_iter()
                    .map(|(symbol, interface_name)| {
                        (
                            symbol.qualified_name.as_str(),
                            go_interface_exemption_reason(interface_name),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let file_reason = if matches!(ext.parse_outcome, ParseOutcome::Fallback { .. }) {
            Some(
                "Declarations recovered by pattern, no call extraction — \
                 excluded from dead code candidates"
                    .to_string(),
            )
        } else if is_parse_failed {
            Some("Parse failed — excluded from dead code candidates".to_string())
        } else {
            file_wiring.first().map(|w| w.details.clone())
        };

        for sym in &ext.symbols {
            // `starts_with("__")` was also tested here and is subsumed by the
            // single-underscore check — dead code that no mutant could kill.
            if sym.kind == SymbolKind::File || sym.name.starts_with('_') {
                continue; // File nodes and underscore-private symbols are exempt
            }

            let is_called = called_symbols.contains(&(ext.file_path.clone(), sym.name.clone()))
                || called_symbols.contains(&(ext.file_path.clone(), sym.qualified_name.clone()));
            let is_ambiguously_called = ambiguous_symbols
                .contains(&(ext.file_path.clone(), sym.name.clone()))
                || ambiguous_symbols.contains(&(ext.file_path.clone(), sym.qualified_name.clone()));

            let overlaps_parse_error = match &ext.parse_outcome {
                ParseOutcome::Partial { error_ranges } => error_ranges.iter().any(|range| {
                    sym.span.start_byte < range.end_byte && range.start_byte < sym.span.end_byte
                }),
                // No grammar ran, so there are no error ranges to overlap.
                // The file is exempt wholesale via `is_parse_failed` above.
                ParseOutcome::Clean
                | ParseOutcome::Failed { .. }
                | ParseOutcome::Fallback { .. } => false,
            };

            let is_exported = sym.is_exported;
            // Computed before the borrowed chain below so the owned string
            // outlives it.
            let heritage_exemption = reached_through_a_supertype(
                &ext.file_path,
                &dead_symbol_identity(sym, &ext.file_path),
                &supertypes,
                &called_symbols,
            );
            let symbol_exemption: Option<&str> = symbol_exemptions
                .get(sym.qualified_name.as_str())
                .copied()
                .or(heritage_exemption.as_deref())
                .or_else(|| {
                    go_interface_exemptions
                        .get(sym.qualified_name.as_str())
                        .map(String::as_str)
                })
                // A definition whose name a header publishes is this unit's
                // public API, and its callers can lie outside the corpus
                // entirely — a library, a foreign-language binding, hand-written
                // assembly. Keyed on the bare name because that is what a
                // prototype declares; the qualified name belongs to the file
                // that defines it and no header could ever match it.
                .or_else(|| {
                    (is_c_family_language(&ext.language)
                        && !is_c_header_path(&ext.file_path)
                        && c_header_exports.contains(sym.name.as_str()))
                    .then_some("Declared in a C-family header — public interface")
                })
                // A spurious ambiguity, not a real one: the candidates the
                // resolver could not choose between are one identity compiled
                // for different platforms, so the call reached whichever one
                // this build selected.
                //
                // Gated on `is_ambiguously_called` deliberately. If *nothing*
                // calls the identity it is dead in every variant, and the
                // confident branch must keep saying so — a build constraint
                // explains an ambiguity, never an absence of callers.
                .or_else(|| {
                    (is_ambiguously_called
                        && go_package_key(ext)
                            .map(|(dir, package)| {
                                go_build_variants.contains(&(
                                    dir,
                                    package,
                                    dead_symbol_identity(sym, &ext.file_path),
                                ))
                            })
                            .unwrap_or(false))
                    .then_some(GO_BUILD_VARIANT_REASON)
                });

            if !is_called
                && is_ambiguously_called
                && !is_exported
                && !is_file_exempt
                && symbol_exemption.is_none()
                && !overlaps_parse_error
            {
                reports.push(DeadSymbolReport {
                    symbol_name: dead_symbol_identity(sym, &ext.file_path),
                    file_path: ext.file_path.clone(),
                    confidence: coverage.cap(0.4),
                    is_exempt: false,
                    exemption_reason: Some("only_ambiguous_callers".to_string()),
                });
            } else if !is_called
                && !is_exported
                && !is_file_exempt
                && symbol_exemption.is_none()
                && !overlaps_parse_error
                && unresolved_names.contains(sym.name.as_str())
            {
                // The defect ledger, read at last. Same tier as
                // `only_ambiguous_callers` — both mean "there is evidence
                // something reaches this and we could not prove which" — but a
                // distinct reason, because the two are different evidence and
                // `only_ambiguous_callers` is a machine token three tests match
                // exactly.
                //
                // Placed after the ambiguity branch so a symbol with both keeps
                // the older, more specific token rather than silently changing
                // what those tests observe.
                reports.push(DeadSymbolReport {
                    symbol_name: dead_symbol_identity(sym, &ext.file_path),
                    file_path: ext.file_path.clone(),
                    confidence: coverage.cap(0.4),
                    is_exempt: false,
                    exemption_reason: Some(UNRESOLVED_NAMESAKE_REASON.to_string()),
                });
            } else if !is_called
                && !is_exported
                && !is_file_exempt
                && symbol_exemption.is_none()
                && !overlaps_parse_error
            {
                // The corpus-level half of X6. A confident finding here means
                // "no edge in the whole generation names this symbol" — which
                // is only evidence when every file got to contribute its edges.
                // While it did not, the finding stays visible (hiding it would
                // be its own lie) but must not reach the tier `CLAUDE.md` tells
                // agents to act on.
                reports.push(DeadSymbolReport {
                    symbol_name: dead_symbol_identity(sym, &ext.file_path),
                    file_path: ext.file_path.clone(),
                    confidence: coverage.cap(0.9),
                    is_exempt: false,
                    // Most specific reason wins, matching the exempt branch
                    // below. A symbol in a call-blind file is not merely
                    // downstream of somebody else's coverage hole — its own
                    // file is the hole, and the reader's next move differs:
                    // corpus loss invites a re-index, a missing extractor does
                    // not.
                    exemption_reason: if file_is_call_blind {
                        Some(CALL_BLIND_REASON.to_string())
                    } else if coverage.is_complete() {
                        None
                    } else {
                        Some(COVERAGE_LOSS_REASON.to_string())
                    },
                });
            } else if !is_called {
                reports.push(DeadSymbolReport {
                    symbol_name: dead_symbol_identity(sym, &ext.file_path),
                    file_path: ext.file_path.clone(),
                    confidence: 0.3,
                    is_exempt: true,
                    // Most specific reason wins, so the report names the check
                    // that actually fired rather than a file-wide blanket.
                    exemption_reason: if overlaps_parse_error {
                        Some("Symbol overlaps a tree-sitter parse error".to_string())
                    } else {
                        symbol_exemption
                            .map(str::to_string)
                            .or_else(|| file_reason.clone())
                            .or_else(|| Some("Exported or exempt".to_string()))
                    },
                });
            }
        }
    }

    LivenessOutcome { reports, coverage }
}

#[cfg(all(test, feature = "parse"))]
mod tests {
    use super::*;

    /// The Go package join key is `(directory, package clause)`, and the
    /// directory half must be real.
    ///
    /// Mutation testing replaced `parent_dir` with a constant without any test
    /// noticing. A constant directory makes every Go file in the repository
    /// look like one package, so an interface declared anywhere would exempt a
    /// same-named method everywhere — silently disabling dead-method detection
    /// across the language.
    #[test]
    fn parent_dir_is_the_real_directory() {
        assert_eq!(parent_dir("pkg/svc/node.go"), "pkg/svc");
        assert_eq!(parent_dir("node.go"), "", "a root file has no directory");
        assert_eq!(parent_dir("a/b/c/d.go"), "a/b/c");
        // Two files in different directories must not share a key.
        assert_ne!(parent_dir("a/x.go"), parent_dir("b/x.go"));
    }

    fn symbol(name: &str, start: usize, end: usize) -> ExtractedSymbol {
        ExtractedSymbol {
            name: name.to_string(),
            qualified_name: format!("f.py::{name}"),
            kind: SymbolKind::Function,
            span: Span {
                start_byte: start,
                end_byte: end,
            },
            is_exported: false,
            docstring: None,
            signature: None,
            parent_symbol: None,
            body_signature: None,
            declaration_hash: None,
        }
    }

    fn extraction(symbols: Vec<ExtractedSymbol>, error: Option<(usize, usize)>) -> Extraction {
        let mut ext = devmap_extract::extract_file("f.py", "def x(): pass\n");
        ext.symbols = symbols;
        ext.wiring = Vec::new();
        ext.parse_outcome = match error {
            Some((start_byte, end_byte)) => ParseOutcome::Partial {
                error_ranges: vec![TextRange {
                    start_byte,
                    end_byte,
                }],
            },
            None => ParseOutcome::Clean,
        };
        ext
    }

    fn reports_with(ext: Extraction, edges: Vec<ResolvedEdge>) -> Vec<DeadSymbolReport> {
        let resolution = ResolutionResult {
            edges,
            receiver_types: std::collections::BTreeMap::new(),
            reexport_chains: std::collections::BTreeMap::new(),
            unresolved: Vec::new(),
        };
        analyze_liveness(std::slice::from_ref(&ext), &resolution)
    }

    fn call_edge(target_symbol: &str) -> ResolvedEdge {
        ResolvedEdge {
            source_file: "f.py".to_string(),
            target_file: "f.py".to_string(),
            source_symbol: "f.py::caller".to_string(),
            target_symbol: target_symbol.to_string(),
            edge_kind: devmap_extract::model::EdgeKind::Calls,
            confidence: devmap_extract::model::Confidence::DETERMINISTIC,
            resolution: None,
            details: None,
            evidence: None,
        }
    }

    /// A call marks a symbol live whether it names the bare or qualified form.
    ///
    /// The liveness lookup is a disjunction over both spellings, and flipping it
    /// to a conjunction survived: it would require an edge to name the symbol
    /// *both* ways at once, so essentially every symbol would report as dead.
    #[test]
    fn either_spelling_of_a_call_target_marks_a_symbol_live() {
        for target in ["helper", "f.py::helper"] {
            let out = reports_with(
                extraction(vec![symbol("helper", 0, 10)], None),
                vec![call_edge(target)],
            );
            assert!(
                out.iter()
                    .all(|r| !r.symbol_name.contains("helper") || r.is_exempt),
                "a call naming `{target}` must mark the symbol live: {out:?}"
            );
        }

        // And with no call at all it must still be reported, or the assertion
        // above would pass for the wrong reason.
        let uncalled = reports_with(extraction(vec![symbol("helper", 0, 10)], None), Vec::new());
        assert!(
            uncalled
                .iter()
                .any(|r| r.symbol_name.contains("helper") && !r.is_exempt),
            "an uncalled symbol must be reported: {uncalled:?}"
        );
    }

    /// An *ambiguous* call is recognised under either spelling too.
    ///
    /// `is_ambiguously_called` is a separate disjunction from `is_called`, and
    /// its `||` was still mutable after the `is_called` case was covered.
    /// Collapsing it to `&&` loses the `only_ambiguous_callers` tier: a symbol
    /// whose only inbound callers are ambiguous would be reported as
    /// *confidently* dead rather than at 0.4, which is precisely the
    /// mislabelling that tier exists to prevent.
    #[test]
    fn either_spelling_of_an_ambiguous_call_downgrades_the_verdict() {
        for target in ["helper", "f.py::helper"] {
            let mut ambiguous = call_edge(target);
            ambiguous.confidence = devmap_extract::model::Confidence::SPECULATIVE;
            ambiguous.resolution = Some(std::sync::Arc::new(
                devmap_resolve::model::Resolution::AmbiguousGlobal {
                    candidates: vec![("f.py".to_string(), "helper".to_string())],
                    family: devmap_resolve::model::LangFamily::Python,
                },
            ));
            let out = reports_with(
                extraction(vec![symbol("helper", 0, 10)], None),
                vec![ambiguous],
            );
            let report = out
                .iter()
                .find(|r| r.symbol_name.contains("helper"))
                .unwrap_or_else(|| panic!("helper must be reported for `{target}`: {out:?}"));
            assert!(
                report.confidence <= 0.4,
                "an ambiguously-called symbol must not be confidently dead under \
                 spelling `{target}`, got {report:?}"
            );
        }
    }

    fn reports(ext: Extraction) -> Vec<DeadSymbolReport> {
        let resolution = ResolutionResult {
            edges: Vec::new(),
            receiver_types: std::collections::BTreeMap::new(),
            reexport_chains: std::collections::BTreeMap::new(),
            unresolved: Vec::new(),
        };
        analyze_liveness(std::slice::from_ref(&ext), &resolution)
    }

    /// Underscore-private symbols are skipped; ordinary ones are not.
    ///
    /// The skip is a disjunction and both halves were mutable without a
    /// failure. Collapsing it either reports every private helper as dead or
    /// reports nothing at all.
    #[test]
    fn underscore_private_symbols_are_skipped_and_others_are_not() {
        let out = reports(extraction(
            vec![symbol("_private", 0, 10), symbol("visible", 20, 30)],
            None,
        ));
        assert!(
            !out.iter().any(|r| r.symbol_name.contains("_private")),
            "an underscore-private symbol must not be reported at all: {out:?}"
        );
        assert!(
            out.iter().any(|r| r.symbol_name.contains("visible")),
            "an uncalled public symbol must still be reported: {out:?}"
        );
    }

    /// Span overlap with a parse error is half-open, and the boundary matters.
    ///
    /// X6 exempts symbols overlapping an error range, because a symbol parsed
    /// out of broken source is not evidence of anything. Both `<` comparisons
    /// were mutable to `<=`, which would make a symbol merely *adjacent* to an
    /// error range exempt — quietly suppressing real findings next to any
    /// syntax error.
    #[test]
    fn parse_error_overlap_is_half_open_at_both_ends() {
        // Symbol [20,30) ends exactly where the error range begins: no overlap.
        let touching_before = reports(extraction(vec![symbol("before", 20, 30)], Some((30, 40))));
        assert!(
            touching_before
                .iter()
                .any(|r| r.symbol_name.contains("before") && !r.is_exempt),
            "a symbol ending exactly at an error range does not overlap it: {touching_before:?}"
        );

        // Symbol [40,50) begins exactly where the error range ends: no overlap.
        let touching_after = reports(extraction(vec![symbol("after", 40, 50)], Some((30, 40))));
        assert!(
            touching_after
                .iter()
                .any(|r| r.symbol_name.contains("after") && !r.is_exempt),
            "a symbol starting exactly at an error range end does not overlap it: {touching_after:?}"
        );

        // Genuine overlap must be exempt.
        let overlapping = reports(extraction(vec![symbol("inside", 32, 38)], Some((30, 40))));
        assert!(
            overlapping
                .iter()
                .all(|r| !r.symbol_name.contains("inside") || r.is_exempt),
            "a symbol inside an error range must be exempt: {overlapping:?}"
        );
    }
}
