//! The labeller: for every site, ask a [`Locator`] where its callee is defined
//! and turn the answer into a label, an unknown, or a skip. Nothing is dropped
//! silently: every enumerated site lands in exactly one bucket of the
//! [`Report`], and a caller with any site that is not a label or a stated
//! "defined elsewhere" is left out of the oracle's covered callers.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use devmap_resolve::lsp::{location_byte_in_source, LineIndex, LspAnswer, LspQuery};
use serde::Serialize;

use crate::classify::classify;
use crate::labels::{Labels, Oracle, RecallLabel, Use};
use crate::namer::{Namer, Target};
use crate::sites::{operator_candidates, scan, FileFacts, Site, SiteKind};

const LANGUAGE: &str = "rust";

/// How many unknown sites a report names. The counts are never capped; only the
/// list of examples is.
const SAMPLE_CAP: usize = 50;

/// Why a query got no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocateFailure {
    /// The server refused or failed this request; the session is still usable.
    Request(String),
    /// The session ended (timeout, exit, oversized or malformed frame).
    Fatal(String),
}

/// Whatever answers "where is the thing at this position defined".
pub trait Locator {
    fn locate(
        &mut self,
        query: LspQuery,
        file: &str,
        source: &str,
        line: u32,
        character: u32,
    ) -> Result<LspAnswer, LocateFailure>;
}

/// Bounds on one run.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Most definition requests the run will make. Sites past it are counted
    /// as skipped and their callers are not covered.
    pub max_sites: usize,
    /// No request is started after this instant.
    pub deadline: Instant,
    /// Label functions used as values as well as calls.
    pub references: bool,
    /// Record one line per site saying what became of it, for review.
    pub trace: bool,
}

/// What a run did, in counts that add up: `sites_enumerated` is `labelled +
/// external + sum(unknown) + sum(skipped)`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub server: Option<String>,
    pub server_health: Option<String>,
    pub files_read: usize,
    pub files_unparsed: usize,
    pub files_with_syntax_errors: usize,
    pub sites_enumerated: usize,
    /// Definition requests actually sent.
    pub asked: usize,
    pub implementation_queries: usize,
    /// Declaration requests sent, one per trait-implementing method bound.
    pub declaration_queries: usize,
    /// Sites that became a `recall` label.
    pub labelled: usize,
    /// Answered, and the definition is outside the repository: no label, and
    /// not a reason to distrust the caller.
    pub external: usize,
    /// Sites no label could be made for, by reason: `no_answer`,
    /// `request_error`, `session_lost`, `deadline`, `target_unnamed`,
    /// `mixed_targets`, `impls_unanswered`, `undecodable`,
    /// `operator_unresolved`.
    pub unknown: BTreeMap<String, usize>,
    /// Sites deliberately not labelled, by reason: `not_in_repo_name`,
    /// `non_function_target`, `over_cap`.
    pub skipped: BTreeMap<String, usize>,
    /// Calls the grammar could not turn into a site (`(self.f)(x)`, `f()()`).
    pub unsupported_callees: usize,
    pub macro_invocations: usize,
    /// Invocations of a macro the corpus itself defines with `macro_rules!`:
    /// their expansion calls things nobody wrote at the site, so their
    /// callers are not covered.
    pub user_macro_invocations: usize,
    pub opaque_macro_sites: usize,
    /// Functions the corpus declares under a name a std trait calls
    /// implicitly (`drop`, `from`, `next`, `fmt`, ...), by name. DevMap edges
    /// to them that rest on an implicit call (scope-end `Drop`, `.into()`,
    /// `for`, `{}`) are judged wrong by this oracle without being wrong, so
    /// precision over a corpus that has them is a lower bound.
    pub implicit_callee_names: BTreeMap<String, usize>,
    pub covered_callers: usize,
    pub uncovered_callers: usize,
    /// Why the session ended early, when it did.
    pub session_failure: Option<String>,
    /// Up to 50 unknown sites, `file:line name reason`.
    pub unknown_sample: Vec<String>,
}

/// The labels and the account of how they were made.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub labels: Labels,
    pub report: Report,
    /// One `file:line name outcome` per site, when [`Limits::trace`] is set.
    pub trace: Vec<String>,
}

/// Names a std trait's method is called under without the call being written:
/// scope-end `Drop`, `.into()`, `for`, `{}`, `==`, deref coercion, `?`.
const IMPLICIT_NAMES: &[&str] = &[
    "drop",
    "from",
    "try_from",
    "into",
    "try_into",
    "next",
    "into_iter",
    "fmt",
    "deref",
    "deref_mut",
    "default",
    "clone",
    "eq",
    "ne",
    "cmp",
    "partial_cmp",
    "hash",
    "as_ref",
    "as_mut",
    "borrow",
    "from_str",
    "to_string",
    "from_iter",
    "extend",
    "poll",
];

/// One name per corpus function (and per `use ... as`): a site that spells
/// none of them cannot reach a function in the repository.
fn in_repo_names(facts: &BTreeMap<&str, FileFacts>) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for f in facts.values() {
        names.extend(f.fn_decls.values().map(|d| d.name.clone()));
        names.extend(f.aliases.iter().cloned());
    }
    names
}

fn names_a_repo_function(site: &Site, names: &BTreeSet<String>) -> bool {
    match site.kind {
        SiteKind::Call | SiteKind::Value => names.contains(&site.name),
        SiteKind::Operator => operator_candidates(&site.name)
            .iter()
            .any(|candidate| names.contains(*candidate)),
    }
}

struct Run<'a> {
    files: BTreeMap<&'a str, &'a str>,
    facts: BTreeMap<&'a str, FileFacts>,
    namer: Namer,
    locator: &'a mut dyn Locator,
    limits: &'a Limits,
    report: Report,
    uncovered: BTreeSet<String>,
    dead_session: bool,
    recall: Vec<RecallLabel>,
    uses: BTreeMap<(String, String), Vec<String>>,
    trace: Option<Vec<String>>,
    /// `file:line name` of the site being decided, when tracing.
    at: String,
    /// Implementation answers by trait method declaration, so a trait method
    /// called from many places is asked about once.
    impls: BTreeMap<(String, usize), Result<Vec<Target>, &'static str>>,
    /// Trait declarations by implementing method, for the same reason.
    decls: BTreeMap<(String, usize), Option<Target>>,
}

/// A function a site was bound to.
struct Bound {
    target: Target,
    /// Its declaration's file and name byte.
    at: (String, usize),
    /// Set when the function is a trait method's declaration.
    trait_decl: Option<(String, usize)>,
}

/// Label every site of `files` (`(path, source)` pairs, DevMap's relative
/// paths) with `locator`.
pub fn label(files: &[(String, String)], locator: &mut dyn Locator, limits: &Limits) -> Outcome {
    let mut sorted: Vec<&(String, String)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut report = Report {
        files_read: sorted.len(),
        ..Report::default()
    };
    let mut facts = BTreeMap::new();
    let mut unparsed = Vec::new();
    for (path, source) in &sorted {
        match scan(source, limits.references) {
            Ok(f) => {
                report.files_with_syntax_errors += usize::from(f.has_error);
                report.macro_invocations += f.macro_invocations;
                report.unsupported_callees += f.unsupported_callees.len();
                facts.insert(path.as_str(), f);
            }
            Err(_) => {
                report.files_unparsed += 1;
                unparsed.push(path.as_str());
            }
        }
    }
    for f in facts.values() {
        for decl in f.fn_decls.values() {
            if IMPLICIT_NAMES.contains(&decl.name.as_str()) {
                *report
                    .implicit_callee_names
                    .entry(decl.name.clone())
                    .or_default() += 1;
            }
        }
    }
    let owned: Vec<(String, String)> = sorted.iter().map(|p| (p.0.clone(), p.1.clone())).collect();
    let mut run = Run {
        files: sorted
            .iter()
            .map(|p| (p.0.as_str(), p.1.as_str()))
            .collect(),
        namer: Namer::new(&owned),
        facts,
        locator,
        limits,
        report,
        uncovered: BTreeSet::new(),
        dead_session: false,
        recall: Vec::new(),
        uses: BTreeMap::new(),
        trace: limits.trace.then(Vec::new),
        at: String::new(),
        impls: BTreeMap::new(),
        decls: BTreeMap::new(),
    };
    for path in unparsed {
        run.uncover_file(path);
    }
    run.uncover_unreadable_sites();
    run.ask_every_site();
    run.finish()
}

impl Run<'_> {
    fn uncover_file(&mut self, path: &str) {
        self.uncovered.insert(path.to_string());
        for id in self.namer.callers_in(path) {
            self.uncovered.insert(id.to_string());
        }
    }

    /// Sites the walk could not read are not labels, and their callers are not
    /// fully read either.
    fn uncover_unreadable_sites(&mut self) {
        let names = in_repo_names(&self.facts);
        let mut doubtful: Vec<(String, usize)> = Vec::new();
        let mut whole_files: Vec<String> = Vec::new();
        let mut opaque = 0;
        for (path, f) in &self.facts {
            if f.has_error {
                whole_files.push((*path).to_string());
            }
            doubtful.extend(
                f.unsupported_callees
                    .iter()
                    .map(|b| ((*path).to_string(), *b)),
            );
            // A macro that spells the name of a corpus function may call it or
            // name it, in tokens nothing parsed.
            for (byte, idents) in &f.macro_idents {
                if idents.iter().any(|i| names.contains(i)) {
                    opaque += 1;
                    doubtful.push(((*path).to_string(), *byte));
                }
            }
        }
        self.report.opaque_macro_sites = opaque;
        for path in whole_files {
            self.uncover_file(&path);
        }
        let defined: BTreeSet<&str> = self
            .facts
            .values()
            .flat_map(|f| f.macro_defs.iter().map(String::as_str))
            .collect();
        let mut user_invocations = 0;
        for (path, f) in &self.facts {
            for (byte, name) in &f.macro_calls {
                if defined.contains(name.as_str()) {
                    user_invocations += 1;
                    doubtful.push(((*path).to_string(), *byte));
                }
            }
        }
        self.report.user_macro_invocations = user_invocations;
        for (path, byte) in doubtful {
            let caller = self.namer.caller_at(&path, byte);
            self.uncovered.insert(caller);
        }
    }

    fn note(&mut self, outcome: &str) {
        if let Some(trace) = &mut self.trace {
            trace.push(format!("{} {outcome}", self.at));
        }
    }

    fn unknown(&mut self, reason: &str, path: &str, source: &str, site: &Site, caller: &str) {
        self.note(&format!("unknown:{reason}"));
        *self.report.unknown.entry(reason.to_string()).or_default() += 1;
        self.uncovered.insert(caller.to_string());
        if self.report.unknown_sample.len() < SAMPLE_CAP {
            let line = source[..site.query_byte.min(source.len())]
                .matches('\n')
                .count()
                + 1;
            self.report
                .unknown_sample
                .push(format!("{path}:{line} {} {reason}", site.name));
        }
    }

    fn skip(&mut self, reason: &str) {
        self.note(&format!("skipped:{reason}"));
        *self.report.skipped.entry(reason.to_string()).or_default() += 1;
    }

    fn ask_every_site(&mut self) {
        let names = in_repo_names(&self.facts);
        let paths: Vec<&str> = self.facts.keys().copied().collect();
        for path in paths {
            let source = self.files[path];
            let sites = self.facts[path].sites.clone();
            let index = LineIndex::new(source);
            for site in &sites {
                self.report.sites_enumerated += 1;
                let caller = self.namer.caller_at(path, site.site_byte);
                if self.trace.is_some() {
                    let line = source[..site.query_byte.min(source.len())]
                        .matches('\n')
                        .count()
                        + 1;
                    self.at = format!("{path}:{line} {}", site.name);
                }
                if !names_a_repo_function(site, &names) {
                    self.skip("not_in_repo_name");
                    continue;
                }
                if self.dead_session {
                    self.unknown("session_lost", path, source, site, &caller);
                    continue;
                }
                if Instant::now() >= self.limits.deadline {
                    self.unknown("deadline", path, source, site, &caller);
                    continue;
                }
                if self.report.asked >= self.limits.max_sites {
                    self.skip("over_cap");
                    self.uncovered.insert(caller);
                    continue;
                }
                self.report.asked += 1;
                let (line, character) = index.position(source, site.query_byte);
                let answer =
                    self.locator
                        .locate(LspQuery::Definition, path, source, line, character);
                self.decide(path, source, site, &caller, answer);
            }
        }
    }

    /// The in-repo function the location is the declaration of, or why not.
    fn resolve(&self, file: &str, line: u32, character: u32) -> Resolved {
        let (Some(source), Some(facts)) = (self.files.get(file), self.facts.get(file)) else {
            return Resolved::Unnamed;
        };
        let byte = location_byte_in_source(source, line, character);
        let Some(decl) = facts.fn_decls.get(&byte) else {
            return Resolved::NotAFunction;
        };
        match self.namer.target_at(file, byte) {
            Some(target) => Resolved::Function {
                target,
                name: decl.name.clone(),
                at: (file.to_string(), byte),
                trait_decl: decl.in_trait.then(|| (file.to_string(), byte)),
            },
            None => Resolved::Unnamed,
        }
    }

    fn decide(
        &mut self,
        path: &str,
        source: &str,
        site: &Site,
        caller: &str,
        answer: Result<LspAnswer, LocateFailure>,
    ) {
        let answer = match answer {
            Ok(a) => a,
            Err(LocateFailure::Request(_)) => {
                return self.unknown("request_error", path, source, site, caller)
            }
            Err(LocateFailure::Fatal(why)) => {
                self.dead_session = true;
                self.report.session_failure = Some(why);
                return self.unknown("session_lost", path, source, site, caller);
            }
        };
        if answer.locations.is_empty() {
            if answer.outside_repo > 0 {
                self.note("external");
                self.report.external += 1;
            } else if answer.undecodable > 0 {
                self.unknown("undecodable", path, source, site, caller);
            } else {
                self.unknown("no_answer", path, source, site, caller);
            }
            return;
        }
        let mut functions: Vec<Bound> = Vec::new();
        let (mut not_function, mut unnamed) = (0usize, 0usize);
        let mut names: Vec<String> = Vec::new();
        for location in &answer.locations {
            match self.resolve(&location.file, location.line, location.character) {
                Resolved::Function {
                    target,
                    name,
                    at,
                    trait_decl,
                } => {
                    names.push(name);
                    functions.push(Bound {
                        target,
                        at,
                        trait_decl,
                    });
                }
                Resolved::NotAFunction => not_function += 1,
                Resolved::Unnamed => unnamed += 1,
            }
        }
        if unnamed > 0 {
            return self.unknown("target_unnamed", path, source, site, caller);
        }
        // rust-analyzer answers for the token at an offset, and an operator
        // sits against its operands: `t[0]` is asked at `[`, between `t` and
        // `0`, and the answer is `t`'s declaration. An operator's answer counts
        // only when it is a function the operator's trait names; anything else
        // is not a statement about the operator.
        if site.kind == SiteKind::Operator {
            let wanted = operator_candidates(&site.name);
            if functions.is_empty()
                || not_function > 0
                || names.iter().any(|n| !wanted.contains(&n.as_str()))
            {
                return self.unknown("operator_unresolved", path, source, site, caller);
            }
        }
        if functions.is_empty() {
            if self.trace.is_some() {
                let named: Vec<String> = answer
                    .locations
                    .iter()
                    .map(|l| format!("{}:{}:{}", l.file, l.line + 1, l.character))
                    .collect();
                self.at = format!("{} -> {}", self.at, named.join(","));
            }
            return self.skip("non_function_target");
        }
        if not_function > 0 {
            return self.unknown("mixed_targets", path, source, site, caller);
        }
        functions.sort_by(|a, b| a.target.id.cmp(&b.target.id));
        functions.dedup_by(|a, b| a.target.id == b.target.id);

        let mut truth: Vec<String> = functions.iter().map(|b| b.target.id.clone()).collect();
        let mut implementations: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for bound in &functions {
            let Some(decl) = &bound.trait_decl else {
                continue;
            };
            match self.implementations_of(decl) {
                Ok(found) => {
                    let ids: BTreeSet<String> = found.iter().map(|t| t.id.clone()).collect();
                    implementations.insert(bound.target.id.clone(), ids.iter().cloned().collect());
                    let fresh: Vec<String> =
                        ids.into_iter().filter(|id| !truth.contains(id)).collect();
                    truth.extend(fresh);
                }
                Err("session_lost") => {
                    return self.unknown("session_lost", path, source, site, caller)
                }
                Err(reason) => return self.unknown(reason, path, source, site, caller),
            }
        }
        // A call bound to a trait implementation is also a call of the trait's
        // method: DevMap, which keeps the written type, may edge to either.
        // This only adds to a label that is already true, so a declaration that
        // cannot be had leaves the strict label.
        for bound in &functions {
            if bound.trait_decl.is_some() || !bound.target.is_method {
                continue;
            }
            if let Some(decl) = self.declaration_of(&bound.at) {
                if decl.id != bound.target.id {
                    if !truth.contains(&decl.id) {
                        truth.push(decl.id.clone());
                    }
                    implementations
                        .entry(bound.target.id.clone())
                        .or_default()
                        .push(decl.id);
                }
            }
        }

        let construct = classify(&site.shape, functions[0].target.is_method);
        self.report.labelled += 1;
        self.note(&format!("{} {}", construct.as_str(), truth.join("|")));
        self.recall.push(RecallLabel {
            language: LANGUAGE.to_string(),
            construct: construct.as_str().to_string(),
            caller: caller.to_string(),
            name: site.name.clone(),
            truth: truth.join("|"),
        });
        for bound in &functions {
            self.uses.insert(
                (caller.to_string(), bound.target.id.clone()),
                implementations.remove(&bound.target.id).unwrap_or_default(),
            );
        }
    }

    /// The trait method an implementation's method implements, asked once per
    /// method. `None` for an inherent method, an answer that names no trait
    /// method of the corpus, or a request that could not be made or answered.
    fn declaration_of(&mut self, at: &(String, usize)) -> Option<Target> {
        if let Some(known) = self.decls.get(at) {
            return known.clone();
        }
        let found = self.ask_declaration(at);
        self.decls.insert(at.clone(), found.clone());
        found
    }

    fn ask_declaration(&mut self, at: &(String, usize)) -> Option<Target> {
        if self.dead_session || Instant::now() >= self.limits.deadline {
            return None;
        }
        let source = self.files[at.0.as_str()];
        let (line, character) = LineIndex::new(source).position(source, at.1);
        self.report.declaration_queries += 1;
        let answer =
            match self
                .locator
                .locate(LspQuery::Declaration, &at.0, source, line, character)
            {
                Ok(answer) => answer,
                Err(LocateFailure::Request(_)) => return None,
                Err(LocateFailure::Fatal(why)) => {
                    self.dead_session = true;
                    self.report.session_failure = Some(why);
                    return None;
                }
            };
        answer.locations.iter().find_map(|location| {
            match self.resolve(&location.file, location.line, location.character) {
                Resolved::Function {
                    target,
                    trait_decl: Some(_),
                    ..
                } => Some(target),
                _ => None,
            }
        })
    }

    /// The in-repo implementations of a trait method declaration, asked once.
    fn implementations_of(&mut self, decl: &(String, usize)) -> Result<Vec<Target>, &'static str> {
        if let Some(known) = self.impls.get(decl) {
            return known.clone();
        }
        let result = self.ask_implementations(decl);
        self.impls.insert(decl.clone(), result.clone());
        result
    }

    fn ask_implementations(&mut self, decl: &(String, usize)) -> Result<Vec<Target>, &'static str> {
        if self.dead_session {
            return Err("session_lost");
        }
        if Instant::now() >= self.limits.deadline {
            return Err("deadline");
        }
        let source = self.files[decl.0.as_str()];
        let (line, character) = LineIndex::new(source).position(source, decl.1);
        self.report.implementation_queries += 1;
        let answer =
            match self
                .locator
                .locate(LspQuery::Implementation, &decl.0, source, line, character)
            {
                Ok(a) => a,
                Err(LocateFailure::Request(_)) => return Err("impls_unanswered"),
                Err(LocateFailure::Fatal(why)) => {
                    self.dead_session = true;
                    self.report.session_failure = Some(why);
                    return Err("session_lost");
                }
            };
        let mut found = Vec::new();
        for location in &answer.locations {
            // An implementation that is not a function in the corpus (an
            // associated type, a generated item) names no callee to accept.
            if let Resolved::Function { target, .. } =
                self.resolve(&location.file, location.line, location.character)
            {
                found.push(target);
            }
        }
        Ok(found)
    }

    fn finish(mut self) -> Outcome {
        self.recall.sort_by(|a, b| {
            (&a.caller, &a.truth, &a.name, &a.construct).cmp(&(
                &b.caller,
                &b.truth,
                &b.name,
                &b.construct,
            ))
        });
        let universe: BTreeSet<&str> = self.namer.callers().collect();
        let covered: Vec<String> = universe
            .iter()
            .filter(|id| !self.uncovered.contains(**id))
            .map(|id| (*id).to_string())
            .collect();
        self.report.covered_callers = covered.len();
        self.report.uncovered_callers = universe.len() - covered.len();
        let covered_set: BTreeSet<&String> = covered.iter().collect();
        let uses = self
            .uses
            .into_iter()
            .filter(|((caller, _), _)| covered_set.contains(caller))
            .map(|((caller, callee), impls)| Use {
                caller,
                callee,
                impls,
            })
            .collect();
        Outcome {
            labels: Labels {
                recall: self.recall,
                oracle: vec![Oracle {
                    language: LANGUAGE.to_string(),
                    covered_callers: covered,
                    uses,
                }],
                dead: Vec::new(),
            },
            report: self.report,
            trace: self.trace.unwrap_or_default(),
        }
    }
}

enum Resolved {
    Function {
        target: Target,
        /// The name the declaration is written with.
        name: String,
        /// The declaration's file and the byte its name starts at.
        at: (String, usize),
        /// The declaration's file and name byte, when it is a trait method.
        trait_decl: Option<(String, usize)>,
    },
    /// The location is not a function's name: a parameter, a local, a type.
    NotAFunction,
    /// It is a function's name, but nothing names it, or the file was never read.
    Unnamed,
}
