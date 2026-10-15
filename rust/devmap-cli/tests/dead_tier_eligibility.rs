//! The confident dead tier is earned per language, by measurement.
//!
//! `devmap_analyze::CONFIDENT_TIER_ELIGIBLE` decides which languages may
//! publish a dead row at 0.9 — the tier `CLAUDE.md` tells agents to act on. A
//! table like that rots the moment it is set by hand: a regression puts one
//! false row into a listed language and nothing notices, or a fix earns a
//! language the tier and nobody lists it. So this file sets it by a check.
//!
//! Every fixture under `testdata/accuracy/<lang>/` is built with the real
//! binary, and its `dead.tsv` truth is applied to the answer:
//!
//! * **A confident-branch row** — published at 0.9, or capped to 0.4 carrying
//!   `confident_tier_unproven` — **not labelled `truly_dead` is a 0.9-tier false
//!   positive.** A language is *proven* by its fixtures when it has at least
//!   one confident row, no false one, and every `truly_dead` marker of the
//!   language reported in the confident branch — at the tier the language is
//!   eligible for, not merely somewhere in the list.
//! * **Listed ⇔ proven**, for every language a fixture exercises; and a listed
//!   language no fixture exercises fails, because nothing proved it.
//! * `testdata/accuracy/record.json`, the labelled corpus record, must exist and
//!   parse; a listed language must have at least one labelled confident row in
//!   it, and be at or under 5% confident-tier false positives on every corpus
//!   it was measured on. A language the record never measured is unproven, not
//!   clean: a check that could not run does not pass.
//!
//! Beside the binding, the truth labels are asserted directly: `truly_dead`
//! symbols are reported, `live` ones never above 0.4, `exempt` ones never
//! reported, and `exported_unreferenced` ones at 0.4 with that token. And the
//! `excluded` disclosure is checked against the exempt rows the store holds.
//!
//! Every reported row must be labelled, so the unconfirmed tier is scored too:
//! `testdata/accuracy/dead_unconfirmed_record.tsv` records, per language, the
//! 0.4 rows and how many of them are `live` or `exempt` (false positives). The
//! 0.4 tier has no gate — it is "unconfirmed" by contract — but its rate is
//! recorded, and a change that moves it fails here until the record is updated
//! in the same change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use devmap_analyze::{
    CONFIDENT_TIER_ELIGIBLE, CONFIDENT_TIER_UNPROVEN, EXPORTED_UNREFERENCED,
    HIGHEST_DEGRADED_CONFIDENCE,
};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// The confidence `confidence_millis` stores for the unconfirmed tier.
const UNCONFIRMED_CEILING: f64 = 0.4;

fn accuracy_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("devmap-cli sits under the workspace root")
        .join("testdata/accuracy")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    TrulyDead,
    Live,
    ExportedUnreferenced,
    Exempt,
}

fn load_truth(fixture: &Path) -> BTreeMap<String, Verdict> {
    let raw = std::fs::read_to_string(fixture.join("dead.tsv"))
        .unwrap_or_else(|e| panic!("{}: {e}", fixture.display()));
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let verdict = match fields.next() {
            Some("truly_dead") => Verdict::TrulyDead,
            Some("live") => Verdict::Live,
            Some("exported_unreferenced") => Verdict::ExportedUnreferenced,
            Some("exempt") => Verdict::Exempt,
            other => panic!(
                "{}: unknown verdict {other:?} in {line:?}",
                fixture.display()
            ),
        };
        let id = fields
            .next()
            .unwrap_or_else(|| panic!("{}: no symbol id in {line:?}", fixture.display()));
        assert!(
            out.insert(id.to_string(), verdict).is_none(),
            "{}: {id} labelled twice",
            fixture.display()
        );
    }
    out
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create fixture copy");
    for entry in std::fs::read_dir(from).expect("read fixture") {
        let entry = entry.expect("fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

struct Built {
    rows: Vec<serde_json::Value>,
    excluded: serde_json::Value,
    db: PathBuf,
}

/// Build one fixture with the real binary, the way an agent's repository is
/// built, and read its whole dead answer.
fn build_fixture(name: &str, fixture: &Path) -> Built {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "devmap-dead-tier-{name}-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    copy_tree(fixture, &repo);
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["init", "-q"])
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");
    let db = root.join("devmap.sqlite");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .arg("--db")
            .arg(&db)
            .arg("--root")
            .arg(&repo)
            .args(args)
            .output()
            .expect("devmap runs");
        assert!(
            out.status.success(),
            "devmap {args:?} on {name}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    run(&["build", "."]);
    let answer: serde_json::Value =
        serde_json::from_str(&run(&["--json", "dead", "--budget", "100000"]))
            .unwrap_or_else(|e| panic!("{name}: dead --json is not JSON: {e}"));
    assert_eq!(
        answer["truncated"], false,
        "{name}: the whole dead list must fit the budget, or this test scores a sample"
    );
    Built {
        rows: answer["items"].as_array().cloned().unwrap_or_default(),
        excluded: answer["excluded"].clone(),
        db,
    }
}

fn language_of(path: &str) -> &'static str {
    devmap_extract::languages::detect_language(Path::new(path))
}

fn reason(row: &serde_json::Value) -> &str {
    row["exemption_reason"].as_str().unwrap_or("")
}

fn confidence(row: &serde_json::Value) -> f64 {
    row["confidence"].as_f64().expect("confidence is a number")
}

/// Whether the evidence earned the confident tier, published or capped.
fn in_confident_branch(row: &serde_json::Value) -> bool {
    confidence(row) > f64::from(HIGHEST_DEGRADED_CONFIDENCE)
        || reason(row).starts_with(CONFIDENT_TIER_UNPROVEN)
}

#[derive(Debug, Default)]
struct LanguageScore {
    confident_rows: usize,
    false_positives: Vec<String>,
    /// `truly_dead` markers reported outside the confident branch: a recall
    /// loss in the tier agents act on, which precision alone never notices.
    missed_markers: Vec<String>,
}

impl LanguageScore {
    fn proven(&self) -> bool {
        self.confident_rows > 0 && self.false_positives.is_empty() && self.missed_markers.is_empty()
    }
}

/// Per language: `(corpus, false_positive, rows)` for each corpus that measured it.
type CorpusRates = BTreeMap<String, Vec<(String, u64, u64)>>;

/// Corpus 0.9-tier false-positive counts from the accuracy harness's record,
/// by language.
///
/// Shape read: `{"corpora": [{"name": …, "languages": {"<lang>": {"dead_fp_by_tier":
/// {"0.9": {"false_positive": n, "rows": m}}}}}]}`. Fails loudly on a missing
/// file, a missing `dead_fp_by_tier` or a malformed `"0.9"` entry: the record is
/// what lets a language publish at 0.9, and a record that cannot be read must
/// not read as one with nothing wrong. A `dead_fp_by_tier` with no `"0.9"` key
/// reads as 0 rows.
fn corpus_record() -> CorpusRates {
    let path = accuracy_root().join("record.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} — the corpus record binds CONFIDENT_TIER_ELIGIBLE",
            path.display()
        )
    });
    let record: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
    corpus_rates(&record, &path.display().to_string())
}

/// [`corpus_record`] over an already-parsed record; `origin` names it in a
/// failure. Split out so the shapes the reader accepts and refuses are tested
/// without writing a record to disk.
fn corpus_rates(record: &serde_json::Value, origin: &str) -> CorpusRates {
    let corpora = record["corpora"]
        .as_array()
        .filter(|corpora| !corpora.is_empty())
        .unwrap_or_else(|| panic!("{origin}: no `corpora` array, or it is empty"));
    let mut by_language: CorpusRates = BTreeMap::new();
    for corpus in corpora {
        let name = corpus["name"]
            .as_str()
            .unwrap_or_else(|| panic!("{origin}: a corpus without a `name`: {corpus}"))
            .to_string();
        let languages = corpus["languages"]
            .as_object()
            .unwrap_or_else(|| panic!("{origin}: {name} has no `languages` object"));
        for (language, entry) in languages {
            // `dead_fp_by_tier` must be there: a record that does not say
            // whether the language was scored is not a record of it. Present
            // with no `"0.9"` key is a measurement - the corpus was labelled and
            // no row of the language reached the confident tier - so it
            // contributes 0 rows, which `corpus_proof` never counts as proof.
            let Some(by_tier) = entry["dead_fp_by_tier"].as_object() else {
                panic!("{origin}: {name}/{language} has no dead_fp_by_tier object");
            };
            let (fp, rows) = match by_tier.get("0.9") {
                None => (0, 0),
                Some(tier) => {
                    let (Some(fp), Some(rows)) =
                        (tier["false_positive"].as_u64(), tier["rows"].as_u64())
                    else {
                        panic!(
                            "{origin}: {name}/{language} has a malformed dead_fp_by_tier.0.9: {tier}"
                        );
                    };
                    (fp, rows)
                }
            };
            by_language
                .entry(language.clone())
                .or_default()
                .push((name.clone(), fp, rows));
        }
    }
    by_language
}

/// What a language's corpus rows add up to: the labelled confident rows across
/// every corpus that measured it, and whether each corpus is at or under 5%
/// false positives. Rows are the proof; a corpus with 0 rows is clean and proves
/// nothing, so the caller still requires `rows > 0`.
fn corpus_proof(corpora: &[(String, u64, u64)]) -> (u64, bool) {
    let rows = corpora.iter().map(|(_, _, rows)| rows).sum();
    let clean = corpora
        .iter()
        .all(|(_, fp, rows)| fp.saturating_mul(20) <= *rows);
    (rows, clean)
}

/// The committed 0.4-tier record: language → `(rows, false_positive)`.
fn unconfirmed_record() -> BTreeMap<String, (u64, u64)> {
    let path = accuracy_root().join("dead_unconfirmed_record.tsv");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let [language, rows, false_positive] = fields.as_slice() else {
            panic!(
                "{}: want language<TAB>rows<TAB>false_positive, got {line:?}",
                path.display()
            );
        };
        let parse = |field: &str| {
            field
                .parse::<u64>()
                .unwrap_or_else(|e| panic!("{}: {field:?} in {line:?}: {e}", path.display()))
        };
        assert!(
            out.insert(language.to_string(), (parse(rows), parse(false_positive)))
                .is_none(),
            "{}: {language} recorded twice",
            path.display()
        );
    }
    out
}

#[test]
fn the_confident_tier_is_listed_exactly_for_the_languages_their_fixtures_prove() {
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(accuracy_root())
        .expect("testdata/accuracy is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("dead.tsv").is_file())
        .collect();
    fixtures.sort();
    assert!(
        fixtures.len() >= 6,
        "expected the six audit fixtures under testdata/accuracy, found {fixtures:?}"
    );

    let mut scores: BTreeMap<&'static str, LanguageScore> = BTreeMap::new();
    let mut unconfirmed: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut violations: Vec<String> = Vec::new();

    for fixture in &fixtures {
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let truth = load_truth(fixture);
        let built = build_fixture(&name, fixture);

        let mut reported: BTreeMap<String, &serde_json::Value> = BTreeMap::new();
        for row in &built.rows {
            let id = format!(
                "{}::{}",
                row["file_path"].as_str().unwrap_or(""),
                row["symbol_name"].as_str().unwrap_or("")
            );
            let language = language_of(row["file_path"].as_str().unwrap_or(""));
            if in_confident_branch(row) {
                let score = scores.entry(language).or_default();
                score.confident_rows += 1;
                if truth.get(&id) != Some(&Verdict::TrulyDead) {
                    score.false_positives.push(format!(
                        "{name}: {id} ({:.2}, {})",
                        confidence(row),
                        reason(row)
                    ));
                }
            }
            if confidence(row) > f64::from(HIGHEST_DEGRADED_CONFIDENCE)
                && !CONFIDENT_TIER_ELIGIBLE.contains(&language)
            {
                violations.push(format!(
                    "{name}: {id} published at {:.2} although {language} is not in \
                     CONFIDENT_TIER_ELIGIBLE",
                    confidence(row)
                ));
            }
            match truth.get(&id) {
                None => violations.push(format!(
                    "{name}: {id} is reported ({:.2}, {}) but not labelled in dead.tsv — \
                     label it, or the tier records score a sample",
                    confidence(row),
                    reason(row)
                )),
                Some(verdict) if confidence(row) <= UNCONFIRMED_CEILING + 1e-6 => {
                    let tally = unconfirmed.entry(language.to_string()).or_default();
                    tally.0 += 1;
                    if matches!(verdict, Verdict::Live | Verdict::Exempt) {
                        tally.1 += 1;
                    }
                }
                Some(_) => {}
            }
            reported.insert(id, row);
        }

        for (id, verdict) in &truth {
            let row = reported.get(id);
            match verdict {
                Verdict::TrulyDead => match row {
                    None => violations.push(format!("{name}: truly dead {id} is not reported")),
                    // Found, but outside the confident branch: the language has
                    // not proven the tier, so a listed one fails below.
                    Some(row) if !in_confident_branch(row) => scores
                        .entry(language_of(id.split("::").next().unwrap_or("")))
                        .or_default()
                        .missed_markers
                        .push(format!(
                            "{name}: {id} ({:.2}, {})",
                            confidence(row),
                            reason(row)
                        )),
                    Some(_) => {}
                },
                Verdict::Live => {
                    if let Some(row) =
                        row.filter(|row| confidence(row) > UNCONFIRMED_CEILING + 1e-6)
                    {
                        violations.push(format!(
                            "{name}: live {id} reported above 0.4 ({:.2}, {})",
                            confidence(row),
                            reason(row)
                        ));
                    }
                }
                Verdict::ExportedUnreferenced => match row {
                    Some(row)
                        if reason(row).starts_with(EXPORTED_UNREFERENCED)
                            && confidence(row) <= UNCONFIRMED_CEILING + 1e-6 =>
                    {
                        let evidence = &row["evidence"];
                        if evidence.get("sites").is_none() {
                            violations
                                .push(format!("{name}: {id} carries no site evidence: {row}"));
                        }
                    }
                    Some(row) => violations.push(format!(
                        "{name}: {id} must be 0.4 {EXPORTED_UNREFERENCED}, got {:.2} {}",
                        confidence(row),
                        reason(row)
                    )),
                    None => violations.push(format!(
                        "{name}: {id} must be listed as {EXPORTED_UNREFERENCED}"
                    )),
                },
                Verdict::Exempt => {
                    if let Some(row) = row {
                        violations.push(format!(
                            "{name}: {id} must never be a candidate, got {:.2} {}",
                            confidence(row),
                            reason(row)
                        ));
                    }
                }
            }
        }

        // Every unconfirmed row says what to check.
        for row in built
            .rows
            .iter()
            .filter(|row| confidence(row) <= UNCONFIRMED_CEILING + 1e-6)
        {
            let evidence = &row["evidence"];
            let Some(sites) = evidence.get("sites").and_then(serde_json::Value::as_u64) else {
                violations.push(format!("{name}: 0.4 row without site evidence: {row}"));
                continue;
            };
            // A named site is a jump target: the path, and the line the query
            // layer read off the file on disk.
            if sites > 0
                && (evidence["first_path"].as_str().is_none()
                    || evidence["first_line"].as_u64().is_none())
            {
                violations.push(format!(
                    "{name}: {sites} site(s) but no first path:line to check: {row}"
                ));
            }
        }

        check_excluded(&name, &built, &mut violations);
    }

    let recorded = unconfirmed_record();
    if recorded != unconfirmed {
        let measured: Vec<String> = unconfirmed
            .iter()
            .map(|(language, (rows, fp))| format!("{language}\t{rows}\t{fp}"))
            .collect();
        violations.push(format!(
            "the 0.4-tier record {recorded:?} no longer matches the fixtures; measured \
             (language, rows, false_positive):\n{}",
            measured.join("\n")
        ));
    }

    let record = corpus_record();
    for (language, score) in &scores {
        let listed = CONFIDENT_TIER_ELIGIBLE.contains(language);
        let corpora = record.get(*language).map(Vec::as_slice).unwrap_or_default();
        let (corpus_rows, corpus_clean) = corpus_proof(corpora);
        // Measured, not merely unrefuted: zero labelled rows is a check that
        // never ran, and passing it would list a language on its fixture alone.
        let proven = score.proven() && corpus_rows > 0 && corpus_clean;
        if listed && !proven {
            violations.push(format!(
                "{language} is in CONFIDENT_TIER_ELIGIBLE but its record is not clean: \
                 {} confident fixture row(s), false positives {:?}, truly-dead markers \
                 outside the confident branch {:?}, {corpus_rows} labelled corpus row(s) \
                 over {} corpora, every corpus within 5%: {corpus_clean}",
                score.confident_rows,
                score.false_positives,
                score.missed_markers,
                corpora.len()
            ));
        }
        if !listed && proven {
            violations.push(format!(
                "{language} earned the confident tier ({} confident row(s), all truly dead) \
                 but is not in CONFIDENT_TIER_ELIGIBLE — list it",
                score.confident_rows
            ));
        }
    }
    for language in CONFIDENT_TIER_ELIGIBLE {
        if !scores.contains_key(language) {
            violations.push(format!(
                "{language} is in CONFIDENT_TIER_ELIGIBLE but no fixture under \
                 testdata/accuracy produces a confident row in it — nothing proves it"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "dead-tier eligibility is not bound to the fixtures:\n  {}\n\nscores: {scores:#?}",
        violations.join("\n  ")
    );
}

/// The `excluded` disclosure equals the exempt rows the store holds, per
/// language — counted here independently, from the stored rows and the
/// extractor's own language detection, not through the store's join.
fn check_excluded(name: &str, built: &Built, violations: &mut Vec<String>) {
    let conn = rusqlite::Connection::open(&built.db).expect("open the fixture store");
    let mut stmt = conn
        .prepare(
            "SELECT file_path FROM generation_dead_symbols
             WHERE generation_id = (SELECT MAX(id) FROM generations) AND is_exempt = 1",
        )
        .expect("prepare");
    let mut expected: BTreeMap<String, u64> = BTreeMap::new();
    for path in stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
    {
        *expected
            .entry(language_of(&path.expect("row")).to_string())
            .or_default() += 1;
    }

    let mut disclosed: BTreeMap<String, u64> = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in built.excluded["languages"].as_array().into_iter().flatten() {
        let language = entry["language"].as_str().unwrap_or("").to_string();
        let not_examined = entry["not_examined"].as_u64().unwrap_or(0);
        let exempt = entry["exempt"].as_u64().unwrap_or(0);
        let by_reason: u64 = entry["by_reason"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, count)| count.as_u64())
            .sum();
        if by_reason != not_examined + exempt {
            violations.push(format!(
                "{name}: excluded[{language}] by_reason sums to {by_reason}, \
                 not not_examined + exempt = {}",
                not_examined + exempt
            ));
        }
        assert!(
            seen.insert(language.clone()),
            "{name}: {language} disclosed twice"
        );
        disclosed.insert(language, not_examined + exempt);
    }
    if disclosed != expected {
        violations.push(format!(
            "{name}: excluded disclosure {disclosed:?} != exempt rows in \
             generation_dead_symbols {expected:?}"
        ));
    }
}

// ---------------------------------------------------- the record reader's shapes

fn record_with(entry: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "corpora": [{ "name": "corpus", "languages": { "lang": entry } }]
    })
}

#[test]
fn a_language_measured_with_no_confident_rows_reads_as_zero_rows() {
    // `dead_fp_by_tier: {}`: the corpus was labelled and no row of the language
    // reached the confident tier. Present, measured, empty - 0 rows.
    let rates = corpus_rates(
        &record_with(serde_json::json!({ "dead_fp_by_tier": {} })),
        "test record",
    );
    assert_eq!(
        rates["lang"],
        vec![("corpus".to_string(), 0, 0)],
        "an empty tier is a measurement of nothing, not a missing one"
    );
    // ... and nothing, however clean, can prove a language eligible.
    let (rows, clean) = corpus_proof(&rates["lang"]);
    assert_eq!((rows, clean), (0, true));
    assert!(
        rows == 0,
        "the proof gate is `rows > 0`; an all-empty language must fail it"
    );
}

#[test]
fn empty_rows_beside_a_corpus_with_rows_leave_the_proof_to_that_corpus() {
    let rates = [
        ("empty".to_string(), 0, 0),
        ("measured".to_string(), 1, 40),
    ];
    assert_eq!(corpus_proof(&rates), (40, true));
    let dirty = [
        ("empty".to_string(), 0, 0),
        ("measured".to_string(), 3, 40),
    ];
    assert_eq!(
        corpus_proof(&dirty),
        (40, false),
        "an empty corpus never excuses a dirty one"
    );
}

#[test]
#[should_panic(expected = "has no dead_fp_by_tier")]
fn a_row_without_dead_fp_by_tier_still_fails() {
    corpus_rates(&record_with(serde_json::json!({})), "test record");
}

#[test]
#[should_panic(expected = "has no dead_fp_by_tier")]
fn a_null_dead_fp_by_tier_still_fails() {
    corpus_rates(
        &record_with(serde_json::json!({ "dead_fp_by_tier": null })),
        "test record",
    );
}

#[test]
#[should_panic(expected = "has no dead_fp_by_tier")]
fn a_dead_fp_by_tier_that_is_not_an_object_still_fails() {
    corpus_rates(
        &record_with(serde_json::json!({ "dead_fp_by_tier": [] })),
        "test record",
    );
}

#[test]
#[should_panic(expected = "dead_fp_by_tier.0.9")]
fn a_null_confident_tier_fails() {
    corpus_rates(
        &record_with(serde_json::json!({ "dead_fp_by_tier": { "0.9": null } })),
        "test record",
    );
}

#[test]
#[should_panic(expected = "dead_fp_by_tier.0.9")]
fn an_empty_confident_tier_object_fails() {
    corpus_rates(
        &record_with(serde_json::json!({ "dead_fp_by_tier": { "0.9": {} } })),
        "test record",
    );
}

#[test]
#[should_panic(expected = "dead_fp_by_tier.0.9")]
fn a_negative_rows_count_fails() {
    corpus_rates(
        &record_with(
            serde_json::json!({ "dead_fp_by_tier": { "0.9": { "false_positive": 0, "rows": -1 } } }),
        ),
        "test record",
    );
}

#[test]
#[should_panic(expected = "dead_fp_by_tier.0.9")]
fn a_float_rows_count_fails() {
    corpus_rates(
        &record_with(
            serde_json::json!({ "dead_fp_by_tier": { "0.9": { "false_positive": 0, "rows": 3.5 } } }),
        ),
        "test record",
    );
}

#[test]
#[should_panic(expected = "dead_fp_by_tier.0.9")]
fn a_confident_tier_missing_its_false_positive_count_fails() {
    corpus_rates(
        &record_with(serde_json::json!({ "dead_fp_by_tier": { "0.9": { "rows": 4 } } })),
        "test record",
    );
}
