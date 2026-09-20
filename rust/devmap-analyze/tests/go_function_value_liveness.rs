//! Liveness for a Go function that is used as a *value* rather than called.
//!
//! `Command{Run: runStatus}` never writes `runStatus(...)`. The extractor records
//! the mention as a bare `ReferenceKind::Name`, which the reference ladder
//! declines by design — binding a bare identifier through the unique-global rung
//! would attach `except Exception as e` to some unrelated `def e`.
//!
//! This file used to say that the refusal also meant "the Go package rung
//! written below it never runs", and treated that as the fixed constraint the
//! exemption below had to work around. It was not fixed, it was an ordering
//! accident: the refusal guards the **global** tier, where a bare name really is
//! weak evidence across the corpus, and it caught the two scope rungs only by
//! sitting above them. Moved below them, `Run: runStatus` resolves to its
//! declaring sibling at `SamePackage`/DETERMINISTIC and the function is live by
//! an edge — which refutes a dead finding rather than withholding one.
//!
//! Measured on this repository before that: `runStatus` and `runDoctor` — both
//! registered as `Run:` handlers in `mapcli.go` — were the only two confident
//! non-exempt dead findings left after the Python alias fix, and both are on a
//! live command path.
//!
//! The exemption is still here and still reachable, on the narrower case the
//! rung cannot answer: a package that declares the name twice, where
//! `same_package_target` abstains rather than choosing.
//! `the_exemption_still_covers_what_the_package_rung_abstains_on` is what keeps
//! this file honest about that, instead of leaving a spared-by branch nothing
//! can trigger.
//!
//! The condition is narrower than "some file names this function", because a
//! mention in the *declaring* file is not declined at all: the same-file rung
//! answers it and the symbol is live by an edge, with no exemption involved.
//! `a_same_file_mention_needs_no_exemption` pins that boundary, so this exemption
//! can never be credited with work the resolver already does.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

fn report_for<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|report| report.symbol_name == symbol)
}

/// The handler declarations. `runStatus` is only ever named as a value;
/// `trulyDead` is named nowhere at all.
const COMMANDS: &str = concat!(
    "package mapcli\n",
    "type Command struct {\n",
    "\tUse string\n",
    "\tRun func(args []string)\n",
    "}\n",
    "func runStatus(args []string) {}\n",
    "func trulyDead(args []string) {}\n",
);

/// The registration site, in a different file of the same package — which is
/// why no import binds the name and the same-file rung cannot see it either.
const REGISTRY: &str = concat!(
    "package mapcli\n",
    "func Commands() []*Command {\n",
    "\treturn []*Command{\n",
    "\t\t{Use: \"status\", Run: runStatus},\n",
    "\t}\n",
    "}\n",
);

#[test]
fn a_go_function_named_as_a_value_is_live_by_an_edge() {
    // Superseded, and by the cause rather than the symptom. The module doc
    // above recorded that the package rung "never runs" for a bare `Name`
    // because the ladder's refusal sat above it; that refusal now sits *below*
    // the two scope rungs, so `Run: runStatus` resolves to the sibling file at
    // `SamePackage`/DETERMINISTIC and the function has a caller.
    //
    // The property this test was written to protect — a function named as a
    // `Run:` value must not be a confident dead finding — is unchanged, and is
    // now met the way `a_same_file_mention_needs_no_exemption` describes as the
    // stronger form: not reported at all, because an edge answers it. An
    // exemption withholds a finding; an edge refutes it.
    let out = reports(&[
        ("cmd/mapcli/commands.go", COMMANDS),
        ("cmd/mapcli/mapcli.go", REGISTRY),
    ]);
    assert!(
        report_for(&out, "runStatus").is_none(),
        "`Run: runStatus` resolves to the declaring sibling; nothing should be \
         reported for it: {out:?}"
    );
}

/// The exemption is still reachable, and this is the case that reaches it.
///
/// With the package rung answering the ordinary case, the question "does
/// `GO_VALUE_MENTION_REASON` still do anything" has to be answered rather than
/// assumed — a spared-by branch nothing can trigger is dead code wearing a
/// justification. It can: `same_package_target` **abstains** when the package
/// declares the name twice (`a_name_the_package_declares_twice_is_an_abstention_not_a_choice`
/// in the resolver's own suite pins that), so the rung returns nothing, the
/// value mention resolves to no edge, and the function would be confidently
/// dead on evidence that does not support it. That is where the exemption
/// still earns its place.
#[test]
fn the_exemption_still_covers_what_the_package_rung_abstains_on() {
    let out = reports(&[
        (
            "cmd/mapcli/commands.go",
            concat!(
                "package mapcli\n",
                "type Command struct {\n",
                "\tRun func(args []string)\n",
                "}\n",
                "func runStatus(args []string) {}\n",
            ),
        ),
        (
            // A second declaration of the same name in the same package: the
            // rung cannot say which is meant, so it says nothing.
            "cmd/mapcli/legacy.go",
            concat!("package mapcli\n", "func runStatus(args []string) {}\n"),
        ),
        ("cmd/mapcli/mapcli.go", REGISTRY),
    ]);
    let status = report_for(&out, "runStatus")
        .unwrap_or_else(|| panic!("`runStatus` must be reported at all: {out:?}"));
    assert!(
        status.is_exempt,
        "the rung abstained, so the mention is all the evidence there is — and \
         withholding the finding is what to do with it: {status:?}"
    );
    assert_eq!(
        status.exemption_reason.as_deref(),
        Some(GO_VALUE_MENTION_REASON),
        "the exemption must say which evidence spared it: {status:?}"
    );
}

#[test]
fn a_go_function_nothing_names_is_still_reported() {
    let out = reports(&[
        ("cmd/mapcli/commands.go", COMMANDS),
        ("cmd/mapcli/mapcli.go", REGISTRY),
    ]);
    let dead = report_for(&out, "trulyDead")
        .unwrap_or_else(|| panic!("`trulyDead` must still be reported: {out:?}"));
    assert!(
        !dead.is_exempt,
        "the exemption must not spread to a function no file names: {dead:?}"
    );
    assert!(
        dead.confidence > 0.5,
        "and it must keep its confidence: {dead:?}"
    );
}

#[test]
fn a_same_file_mention_needs_no_exemption() {
    // The boundary the exemption is defined against. A bare mention in the
    // declaring file is answered by the same-file rung, so the symbol is live by
    // an edge and never becomes a candidate at all. If this ever starts
    // returning an exempt report instead of no report, the exemption has taken
    // over work the resolver was already doing — and the evidence it would then
    // be resting on is the function's own body.
    let out = reports(&[(
        "cmd/mapcli/selfish.go",
        concat!(
            "package mapcli\n",
            "func selfNamed(args []string) {}\n",
            "func Register() interface{} {\n",
            "\treturn selfNamed\n",
            "}\n",
        ),
    )]);
    assert!(
        report_for(&out, "selfNamed").is_none(),
        "a same-file mention resolves; nothing should be reported for it: {out:?}"
    );
}

#[test]
fn a_mention_in_another_package_is_not_evidence() {
    // Go's own visibility rule, which the resolver already enforces in
    // `go_symbol_visible_from`: an unexported name is reachable only from its own
    // directory, so a namesake mention elsewhere says nothing about this
    // function. Keying the exemption corpus-wide would exempt every same-named
    // unexported function in every unrelated package.
    let out = reports(&[
        (
            "cmd/mapcli/commands.go",
            concat!("package mapcli\n", "func runStatus(args []string) {}\n"),
        ),
        (
            "internal/other/register.go",
            concat!(
                "package other\n",
                "func Register() interface{} {\n",
                "\tvar runStatus = 1\n",
                "\treturn runStatus\n",
                "}\n",
            ),
        ),
    ]);
    let dead = report_for(&out, "runStatus")
        .unwrap_or_else(|| panic!("`runStatus` must be reported: {out:?}"));
    assert!(
        !dead.is_exempt,
        "a namesake in an unrelated package must not spare this one: {dead:?}"
    );
}

#[test]
fn a_field_access_in_another_file_is_not_a_value_mention() {
    // `cfg.runDoctor` names a struct field, not this function. The reference
    // carries a receiver, and the exemption requires a bare identifier — so this
    // is the test that keeps `receiver_expr.is_none()` load-bearing rather than
    // decorative. The read lives in a second file of the same package, which is
    // the only place the exemption looks.
    let out = reports(&[
        (
            "cmd/mapcli/fields.go",
            concat!(
                "package mapcli\n",
                "type Config struct {\n",
                "\trunDoctor bool\n",
                "}\n",
                "func runDoctor(args []string) {}\n",
            ),
        ),
        (
            "cmd/mapcli/read.go",
            concat!(
                "package mapcli\n",
                "func Read(cfg Config) bool {\n",
                "\treturn cfg.runDoctor\n",
                "}\n",
            ),
        ),
    ]);
    let dead = report_for(&out, "runDoctor")
        .unwrap_or_else(|| panic!("`runDoctor` must be reported: {out:?}"));
    assert!(
        !dead.is_exempt,
        "a field read must not be mistaken for naming the function: {dead:?}"
    );
}

#[test]
fn a_method_is_not_spared_by_a_bare_namesake() {
    // Restricted to `SymbolKind::Function` on purpose. A method's identity is
    // `Type.Method`, and keying methods on their bare name would exempt every
    // same-named method of every type in the package — measured on this
    // repository, 16 of its 18 Go findings.
    let out = reports(&[
        (
            "cmd/mapcli/server.go",
            concat!(
                "package mapcli\n",
                "type Server struct{}\n",
                "func (s *Server) handle(args []string) {}\n",
            ),
        ),
        (
            "cmd/mapcli/other.go",
            concat!(
                "package mapcli\n",
                "func Register() interface{} {\n",
                "\tvar handle = 1\n",
                "\treturn handle\n",
                "}\n",
            ),
        ),
    ]);
    // Reported under its identity, `Server.handle` — which is also why a bare
    // `handle` could never match it without deliberately stripping the type.
    let dead = report_for(&out, "Server.handle")
        .unwrap_or_else(|| panic!("`Server.handle` must be reported: {out:?}"));
    assert!(
        !dead.is_exempt,
        "a bare namesake must not spare a method: {dead:?}"
    );
}

#[test]
fn a_file_with_no_package_clause_cannot_key_the_join() {
    // `go_package_key` requires a non-empty package clause, so a fragment that
    // declares none contributes no mentions and receives no exemption. Without
    // that filter every such file would share one `("", "")` package with every
    // other, and a mention in any of them would spare a namesake in all of them.
    let out = reports(&[
        (
            "cmd/mapcli/fragment.go",
            "func orphanHandler(args []string) {}\n",
        ),
        (
            "cmd/mapcli/other_fragment.go",
            concat!(
                "func Register() interface{} {\n",
                "\treturn orphanHandler\n",
                "}\n",
            ),
        ),
    ]);
    let dead = report_for(&out, "orphanHandler").unwrap_or_else(|| {
        panic!("`orphanHandler` must be reported, or this test asserts nothing: {out:?}")
    });
    assert_ne!(
        dead.exemption_reason.as_deref(),
        Some(GO_VALUE_MENTION_REASON),
        "a file with no package clause must not join any package: {dead:?}"
    );
}

#[test]
fn the_verdict_does_not_depend_on_the_order_files_arrive_in() {
    // The mention map is a `HashMap`, and the declaring-file exclusion compares
    // against a set. Neither may make the answer depend on which file the walk
    // reached first — a graph whose dead list reshuffles between two runs of the
    // same corpus is not a graph anyone can act on.
    let mut sources: Vec<(&str, &str)> = vec![
        ("cmd/mapcli/commands.go", COMMANDS),
        ("cmd/mapcli/mapcli.go", REGISTRY),
    ];
    // Enough unrelated files, each naming its own handler, that a
    // hash-order-dependent answer has room to show itself.
    let filler: Vec<(String, String)> = (0..64)
        .map(|index| {
            (
                format!("cmd/mapcli/gen_{index}.go"),
                format!(
                    concat!(
                        "package mapcli\n",
                        "func handler{index}(args []string) {{}}\n",
                        "func Wire{index}() interface{{}} {{\n",
                        "\treturn handler{index}\n",
                        "}}\n",
                    ),
                    index = index
                ),
            )
        })
        .collect();
    // Each filler file names its own handler, so the mention is same-file and
    // must resolve rather than be exempted — the fillers are load-bearing noise,
    // not extra subjects.
    for (path, source) in &filler {
        sources.push((path.as_str(), source.as_str()));
    }

    let forward = reports(&sources);
    sources.reverse();
    let reversed = reports(&sources);

    let fingerprint = |rows: &[DeadSymbolReport]| {
        let mut out: Vec<String> = rows
            .iter()
            .map(|row| {
                format!(
                    "{}::{} exempt={} reason={:?} confidence={:.4}",
                    row.file_path,
                    row.symbol_name,
                    row.is_exempt,
                    row.exemption_reason,
                    row.confidence
                )
            })
            .collect();
        out.sort();
        out
    };
    assert_eq!(
        fingerprint(&forward),
        fingerprint(&reversed),
        "the dead list must not depend on file order"
    );
    // The subject resolves now rather than being exempted, so the assertion
    // that keeps this test about something is that it stays resolved — under
    // both orders, with 64 hash-order-shuffling fillers in the corpus.
    assert!(
        report_for(&forward, "runStatus").is_none(),
        "`Run: runStatus` must resolve to its declaring sibling: {forward:?}"
    );
    assert!(
        report_for(&reversed, "runStatus").is_none(),
        "and must still resolve when the files arrive the other way round: {reversed:?}"
    );
    for index in 0..64 {
        assert!(
            report_for(&forward, &format!("handler{index}")).is_none(),
            "a same-file mention resolves, so filler handlers must not be reported: {forward:?}"
        );
    }
}

#[test]
fn a_composite_literal_field_key_does_not_name_a_namesake_function() {
    // The over-approximation this test recorded is gone, and it was closed
    // where its own comment said it belonged — in the extractor. Go spells a
    // struct literal's field key as a bare identifier with no receiver, so
    // `run` in `Command{run: nil}` used to be indistinguishable from naming a
    // package-level `func run`, and an unexported function whose name collided
    // with a field key was spared although nothing used it.
    //
    // `go_struct_literal_field_key` now refuses the key outright: it names a
    // field of the literal's own type, not a symbol. A **map** literal's key is
    // left alone, because there the key really is an expression — the two are
    // separated by the literal's declared type, not by position.
    //
    // Closing it became urgent rather than optional. While the ladder declined
    // every bare `Name` the misreading only withheld a finding; with the Go
    // package rung answering them it would have produced a `SamePackage` edge
    // at DETERMINISTIC instead — a fabricated caller, which is the opposite of
    // the direction an approximation is allowed to be wrong in.
    let out = reports(&[
        (
            "cmd/mapcli/kinds.go",
            concat!(
                "package mapcli\n",
                "type Command struct {\n",
                "\trun func(args []string)\n",
                "}\n",
                "func run(args []string) {}\n",
            ),
        ),
        (
            "cmd/mapcli/build.go",
            concat!(
                "package mapcli\n",
                "func Build() *Command {\n",
                "\treturn &Command{run: nil}\n",
                "}\n",
            ),
        ),
    ]);
    let report =
        report_for(&out, "run").unwrap_or_else(|| panic!("`run` must be reported at all: {out:?}"));
    assert!(
        !report.is_exempt,
        "nothing names this function: a field key is not a mention of it: {report:?}"
    );
    assert!(
        report.confidence > 0.5,
        "and with no mention to withhold on, the finding keeps its confidence: {report:?}"
    );
}

/// A map literal's key is a real expression and must still be one.
///
/// The boundary `go_struct_literal_field_key` is defined against: narrowing the
/// struct case must not take map keys with it, or `map[string]int{limit: 1}`
/// stops naming `limit` and a live constant starts looking dead.
#[test]
fn a_map_literal_key_still_names_what_it_mentions() {
    let out = reports(&[
        (
            "cmd/mapcli/limits.go",
            concat!(
                "package mapcli\n",
                "func limitFor(n int) int { return n }\n"
            ),
        ),
        (
            "cmd/mapcli/table.go",
            concat!(
                "package mapcli\n",
                "func Table() map[int]string {\n",
                "\treturn map[int]string{limitFor(1): \"one\"}\n",
                "}\n",
            ),
        ),
    ]);
    assert!(
        report_for(&out, "limitFor").is_none(),
        "a call in map-key position is still a call: {out:?}"
    );
}
