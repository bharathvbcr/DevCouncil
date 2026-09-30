//! An unexported Go selector names something its own package declares.
//!
//! `job.recordReplayEvent(cancelled)` in scholarlm's `WisDevJobCancelHandler`
//! had no edge: `job` is bound by `job, ok := yoloJobStore.get(jobID)`, and no
//! receiver rung reads a method's return type. The site sat in the ledger as
//! `uninferred_receiver`, and `devmap impact recordReplayEvent` reported the
//! method's other caller alone — so an agent asking "what writes the replay
//! ring?" was told one place when there were two.
//!
//! Go settles it without the type. The compiler refuses an unexported selector
//! from outside the declaring package, even through embedding, so `.m` with a
//! lowercase `m` written in package P is a method, a field or an interface
//! method *of P*. One concrete method and no field or interface method of the
//! name is a proof; anything else is an abstention, and every abstention here
//! has a case.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve_extractions(extractions: &[Extraction]) -> ResolutionResult {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    resolver.resolve_all(extractions).unwrap()
}

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    resolve_extractions(&extractions)
}

/// `(target_symbol, kind)` of every call edge out of `caller` to a symbol
/// whose last segment is `method`.
fn calls_from(
    result: &ResolutionResult,
    caller: &str,
    method: &str,
) -> Vec<(String, ResolutionKind)> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_symbol == caller
                && edge.target_symbol.rsplit(['.', ':']).next() == Some(method)
        })
        .map(|edge| {
            (
                edge.target_symbol.clone(),
                edge.resolution
                    .as_deref()
                    .map(Resolution::kind)
                    .expect("a built edge carries its evidence"),
            )
        })
        .collect()
}

fn ledger_has(result: &ResolutionResult, caller: &str, method: &str) -> bool {
    result
        .unresolved
        .iter()
        .any(|row| row.source_symbol == caller && row.callee_name == method)
}

/// The scholarlm shape, reduced: the job type and its method in one file, a
/// store whose `get` returns it, and a handler in another file of the package
/// that reaches the job only through that return value.
const YOLO: &str = "\
package api

type YoloJob struct {
\treplayBuffer []int
}

func (j *YoloJob) recordReplayEvent(e int) {
\tj.replayBuffer = append(j.replayBuffer, e)
}

type yoloStore struct{}

func (s *yoloStore) get(id string) (*YoloJob, bool) { return nil, false }

var yoloJobStore = &yoloStore{}
";

const HANDLER: &str = "\
package api

func WisDevJobCancelHandler(id string) {
\tif job, ok := yoloJobStore.get(id); ok {
\t\tjob.recordReplayEvent(7)
\t}
}
";

const HANDLER_SYMBOL: &str = "api/handler.go::WisDevJobCancelHandler";

#[test]
fn a_unique_unexported_method_is_reached_through_an_untyped_receiver() {
    let result = resolve(&[("api/yolo.go", YOLO), ("api/handler.go", HANDLER)]);
    assert_eq!(
        calls_from(&result, HANDLER_SYMBOL, "recordReplayEvent"),
        vec![(
            "api/yolo.go::YoloJob.recordReplayEvent".to_string(),
            ResolutionKind::SamePackage
        )],
        "the package declares one `recordReplayEvent` and nothing else of the name"
    );
    assert!(
        !ledger_has(&result, HANDLER_SYMBOL, "recordReplayEvent"),
        "a resolved site must leave the ledger"
    );
}

#[test]
fn a_same_file_untyped_receiver_resolves_too() {
    let source = format!("{YOLO}\nfunc cancel(id string) {{\n\tjob, _ := yoloJobStore.get(id)\n\tjob.recordReplayEvent(1)\n}}\n");
    let result = resolve(&[("api/yolo.go", &source)]);
    assert_eq!(
        calls_from(&result, "api/yolo.go::cancel", "recordReplayEvent").len(),
        1
    );
}

/// Each case adds one declaration to the package that makes `.recordReplayEvent`
/// ambiguous without the receiver's type, and each must leave the site in the
/// ledger rather than pick.
#[test]
fn every_namesake_in_the_package_vetoes_the_answer() {
    let vetoes: &[(&str, &str)] = &[
        (
            "a second concrete method",
            "package api\ntype other struct{}\nfunc (o other) recordReplayEvent(e int) {}\n",
        ),
        (
            "a named interface's method",
            "package api\ntype recorder interface { recordReplayEvent(e int) }\n",
        ),
        (
            "an inline generic constraint",
            "package api\nfunc each[T interface{ recordReplayEvent(int) }](t T) {}\n",
        ),
        (
            "a func-typed struct field",
            "package api\ntype hooks struct { recordReplayEvent func(int) }\n",
        ),
        (
            "a field of an anonymous struct",
            "package api\nvar cfg struct { recordReplayEvent func(int) }\n",
        ),
        (
            "a field sharing a declaration list",
            "package api\ntype pair struct { a, recordReplayEvent func(int) }\n",
        ),
        (
            "an embedded field named by its type",
            "package api\ntype recordReplayEvent func(int)\ntype wrapper struct { *recordReplayEvent }\n",
        ),
    ];
    for (label, extra) in vetoes {
        let result = resolve(&[
            ("api/yolo.go", YOLO),
            ("api/handler.go", HANDLER),
            ("api/extra.go", extra),
        ]);
        assert!(
            calls_from(&result, HANDLER_SYMBOL, "recordReplayEvent").is_empty(),
            "{label}: the rung must abstain"
        );
        assert!(
            ledger_has(&result, HANDLER_SYMBOL, "recordReplayEvent"),
            "{label}: an abstention keeps the ledger row"
        );
    }
}

#[test]
fn an_external_test_package_cannot_see_the_method() {
    let external = "\
package api_test

func TestCancel(t *testing.T) {
\tjob := newJob()
\tjob.recordReplayEvent(1)
}
";
    let result = resolve(&[("api/yolo.go", YOLO), ("api/cancel_test.go", external)]);
    assert!(
        calls_from(
            &result,
            "api/cancel_test.go::TestCancel",
            "recordReplayEvent"
        )
        .is_empty(),
        "`package api_test` is a different package and sees nothing unexported of `api`"
    );
}

#[test]
fn an_internal_test_file_sees_the_method_but_production_does_not_see_test_methods() {
    let internal = "\
package api

func TestCancel(t *testing.T) {
\tjob := newJob()
\tjob.recordReplayEvent(1)
}
";
    let result = resolve(&[("api/yolo.go", YOLO), ("api/cancel_test.go", internal)]);
    assert_eq!(
        calls_from(
            &result,
            "api/cancel_test.go::TestCancel",
            "recordReplayEvent"
        )
        .len(),
        1,
        "a `package api` test file is inside the package"
    );

    let test_only = "package api\ntype fake struct{}\nfunc (f fake) flushOnce() {}\n";
    let production = "package api\nfunc drain(x any) {\n\tv := load(x)\n\tv.flushOnce()\n}\n";
    let result = resolve(&[
        ("api/fake_test.go", test_only),
        ("api/drain.go", production),
    ]);
    assert!(
        calls_from(&result, "api/drain.go::drain", "flushOnce").is_empty(),
        "production code is compiled without `_test.go` files, so their methods are not candidates"
    );
}

#[test]
fn another_directory_is_another_package() {
    let elsewhere = "package api\ntype job struct{}\nfunc (j job) settle() {}\n";
    let caller = "package api\nfunc run(x any) {\n\tv := load(x)\n\tv.settle()\n}\n";
    let result = resolve(&[("other/api/job.go", elsewhere), ("api/run.go", caller)]);
    assert!(calls_from(&result, "api/run.go::run", "settle").is_empty());
}

#[test]
fn an_exported_method_is_never_answered_by_this_rung() {
    let yolo = YOLO.replace("recordReplayEvent", "RecordReplayEvent");
    let handler = HANDLER.replace("recordReplayEvent", "RecordReplayEvent");
    let result = resolve(&[("api/yolo.go", &yolo), ("api/handler.go", &handler)]);
    assert!(
        calls_from(&result, HANDLER_SYMBOL, "RecordReplayEvent").is_empty(),
        "an exported selector can name another package's method; uniqueness here proves nothing"
    );
}

#[test]
fn a_cgo_call_is_not_a_go_method() {
    let yolo = format!("{YOLO}\nfunc (j *YoloJob) strlen() int {{ return 0 }}\n");
    let cgo = "package api\n\nimport \"C\"\n\nfunc measure() {\n\tC.strlen(nil)\n}\n";
    let result = resolve(&[("api/yolo.go", &yolo), ("api/cgo.go", cgo)]);
    assert!(calls_from(&result, "api/cgo.go::measure", "strlen").is_empty());
}

#[test]
fn a_partially_parsed_file_vetoes_only_the_names_it_spells() {
    // Broken, and never mentions the method: the package keeps its answer.
    let unrelated = "package api\n\nfunc broken( {\n";
    let result = resolve(&[
        ("api/yolo.go", YOLO),
        ("api/handler.go", HANDLER),
        ("api/broken.go", unrelated),
    ]);
    assert_eq!(
        calls_from(&result, HANDLER_SYMBOL, "recordReplayEvent").len(),
        1,
        "one broken file must not silence a package for names it never spells"
    );

    // Broken exactly where a second declaration of the name sits.
    let hiding = "package api\n\ntype other struct{}\n\nfunc (o *other) recordReplayEvent( {\n";
    let result = resolve(&[
        ("api/yolo.go", YOLO),
        ("api/handler.go", HANDLER),
        ("api/broken.go", hiding),
    ]);
    assert!(
        calls_from(&result, HANDLER_SYMBOL, "recordReplayEvent").is_empty(),
        "a broken file that spells the name may hold its second declaration"
    );
}

#[test]
fn a_failed_extraction_silences_its_package() {
    let mut extractions: Vec<Extraction> = [
        ("api/yolo.go", YOLO),
        ("api/handler.go", HANDLER),
        ("api/lost.go", "package api\n"),
    ]
    .iter()
    .map(|(path, source)| extract_file(path, source))
    .collect();
    extractions[2].parse_outcome = ParseOutcome::Failed {
        reason: "simulated".to_string(),
    };
    let result = resolve_extractions(&extractions);
    assert!(
        calls_from(&result, HANDLER_SYMBOL, "recordReplayEvent").is_empty(),
        "a file that contributed nothing could declare anything"
    );
}

#[test]
fn the_partial_veto_is_lexical_and_complete() {
    let ext = extract_file(
        "api/broken.go",
        "package api\n\n// flushAll is \"quoted\" _under 9lives\nfunc (o *other) Settle( {\n",
    );
    assert!(matches!(ext.parse_outcome, ParseOutcome::Partial { .. }));
    let names = ext
        .go_member_names
        .expect("a Go extraction always states its list");
    for spelled in [
        "flushAll", "quoted", "_under", "lives", "package", "api", "o", "other",
    ] {
        assert!(
            names.contains(&spelled.to_string()),
            "missing {spelled}: {names:?}"
        );
    }
    assert!(
        !names.contains(&"Settle".to_string()),
        "exported names are never vetoes"
    );
    assert!(
        !names.iter().any(|name| name.starts_with(char::is_numeric)),
        "{names:?}"
    );
}

#[test]
fn an_extraction_cached_before_member_names_existed_abstains() {
    let mut extractions: Vec<Extraction> = [("api/yolo.go", YOLO), ("api/handler.go", HANDLER)]
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    assert!(
        extractions.iter().all(|ext| ext.go_member_names.is_some()),
        "a fresh Go extraction states its member names, even when there are none"
    );
    extractions[0].go_member_names = None;
    let result = resolve_extractions(&extractions);
    assert!(
        calls_from(&result, HANDLER_SYMBOL, "recordReplayEvent").is_empty(),
        "absence of the list is not absence of a namesake"
    );
}

#[test]
fn member_names_are_collected_from_every_literal_and_only_when_unexported() {
    let source = "\
package api

type hooks struct {
\tflush, Close func()
\t*pkg.recorder[int]
\tOuter
}

var anon struct{ drain func() }

type sink interface {
\twrite(b []byte)
\tWrite(b []byte)
}

func each[T interface{ settle() }](t T) {}
";
    let ext = extract_file("api/hooks.go", source);
    assert_eq!(
        ext.go_member_names.as_deref(),
        Some(
            &[
                "drain".to_string(),
                "flush".to_string(),
                "recorder".to_string(),
                "settle".to_string(),
                "write".to_string(),
            ][..]
        ),
    );
    assert_eq!(
        extract_file("app.py", "def f():\n    pass\n").go_member_names,
        None,
        "only Go states the list"
    );
}

#[test]
fn the_partial_veto_survives_hostile_text() {
    // Multibyte identifiers, emoji, a token at end of input, and ~1 MB of
    // distinct names: no panic on a char boundary, nothing dropped.
    let mut source = String::from("package api\n\n// ñame 🚀 émoji_x\nfunc (o *other) Settle( {\n");
    for index in 0..40_000 {
        source.push_str(&format!("// v{index}_x\n"));
    }
    source.push_str("tailtoken");
    let ext = extract_file("api/hostile.go", &source);
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Partial { .. }),
        "{:?} members={:?}",
        ext.parse_outcome,
        ext.go_member_names.as_ref().map(Vec::len)
    );
    let names = ext.go_member_names.unwrap();
    for spelled in ["ñame", "émoji_x", "v0_x", "v39999_x", "tailtoken"] {
        assert!(
            names.binary_search(&spelled.to_string()).is_ok(),
            "missing {spelled}"
        );
    }
    assert!(
        names.windows(2).all(|pair| pair[0] < pair[1]),
        "sorted and deduplicated"
    );
}
