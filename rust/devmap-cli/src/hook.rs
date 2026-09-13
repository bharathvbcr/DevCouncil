//! Host-neutral `devmap hook <event>` entry point.
//!
//! Cursor and Codex run hooks as a shell-string `command` with no `args`/`async`.
//! Exit 2 is treated as "block the agent" on those hosts, so this path never
//! exits 2: success and no-ops are 0, failures are 1.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, bail, Context};
use devmap_extract::subprocess;
use serde_json::{json, Value};

/// Hard cap on hook stdin. Anything larger is treated as malformed and ignored.
pub const MAX_STDIN_BYTES: usize = 1024 * 1024;

/// How long a detached post-tool-use/session-end hook may spend before returning.
pub const DETACH_BUDGET: Duration = Duration::from_millis(100);

/// How many repositories one hook invocation will act on.
///
/// The payload chooses these: `workspace_roots[]` and the paths in an
/// `apply_patch` command are both unbounded arrays, and a repository can carry
/// as many nested store-bearing directories as it likes — `resolve_repo_root`
/// stops at the first store walking up, so each one is its own root. Without a
/// cap, one hook fans out to two child processes per root and the host waits
/// for all of them.
pub const MAX_HOOK_ROOTS: usize = 8;

/// How many payload candidates are examined while looking for those roots.
/// Each candidate costs a canonicalization and a store probe, so the work is
/// bounded even when none of them resolve.
pub const MAX_HOOK_ROOT_CANDIDATES: usize = 256;

/// Total wall clock a synchronous SessionStart may spend in child processes.
/// Hosts block on this hook and paste its stdout into the session, so the
/// budget covers every child together rather than each one separately.
pub const SESSION_START_BUDGET: Duration = Duration::from_secs(10);

/// Bytes kept from one child. The summary keeps a single truncated line; the
/// rest is drained and dropped rather than buffered whole.
const CHILD_OUTPUT_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    PostToolUse,
    SessionEnd,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session-start",
            Self::PostToolUse => "post-tool-use",
            Self::SessionEnd => "session-end",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "session-start" => Some(Self::SessionStart),
            "post-tool-use" => Some(Self::PostToolUse),
            "session-end" => Some(Self::SessionEnd),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct HookOutcome {
    pub exit_code: i32,
    pub stdout: Option<Value>,
    pub stderr_line: Option<String>,
    #[allow(dead_code)]
    pub roots: Vec<PathBuf>,
}

/// Run one hook event. Never returns exit code 2.
pub fn run_hook(
    event: HookEvent,
    stdin: &[u8],
    executable: &Path,
    pinned_root: Option<&Path>,
) -> HookOutcome {
    match run_hook_inner(event, stdin, executable, pinned_root) {
        Ok(outcome) => outcome,
        Err(error) => HookOutcome {
            exit_code: 1,
            stdout: None,
            stderr_line: Some(format!("devmap hook {}: {error}", event.as_str())),
            roots: Vec::new(),
        },
    }
}

fn run_hook_inner(
    event: HookEvent,
    stdin: &[u8],
    executable: &Path,
    pinned_root: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    if stdin.is_empty() {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some("devmap hook: empty stdin; no-op".into()),
            roots: Vec::new(),
        });
    }
    if stdin.len() > MAX_STDIN_BYTES {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some(format!(
                "devmap hook: stdin exceeds {MAX_STDIN_BYTES} bytes; no-op"
            )),
            roots: Vec::new(),
        });
    }
    let text = match std::str::from_utf8(stdin) {
        Ok(text) => text.trim(),
        Err(_) => {
            return Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: Some("devmap hook: stdin is not UTF-8; no-op".into()),
                roots: Vec::new(),
            });
        }
    };
    if text.is_empty() {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some("devmap hook: empty stdin; no-op".into()),
            roots: Vec::new(),
        });
    }
    let payload: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: Some(format!("devmap hook: malformed JSON ({error}); no-op")),
                roots: Vec::new(),
            });
        }
    };

    let selection = select_roots(&payload, pinned_root)?;
    if selection.is_empty() {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some("devmap hook: no indexed repository found; no-op".into()),
            roots: selection.roots,
        });
    }
    let capped_note = selection.capped.then(|| {
        format!(
            "devmap hook: acting on the first {} repository/repositories; the payload named more",
            selection.roots.len()
        )
    });

    match event {
        HookEvent::SessionStart => {
            let stdout = session_start_sync(executable, &selection)?;
            Ok(HookOutcome {
                exit_code: 0,
                stdout: Some(stdout),
                stderr_line: capped_note,
                roots: selection.roots,
            })
        }
        HookEvent::PostToolUse => {
            let budget_note =
                detach_roots(executable, &selection.roots, DETACH_BUDGET, detach_build)?;
            Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: join_notes(capped_note, budget_note),
                roots: selection.roots,
            })
        }
        HookEvent::SessionEnd => {
            let budget_note = detach_roots(
                executable,
                &selection.roots,
                DETACH_BUDGET,
                detach_session_report,
            )?;
            Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: join_notes(capped_note, budget_note),
                roots: selection.roots,
            })
        }
    }
}

/// Repository selection order from the plan, keeping only roots that already
/// hold a store. A hook never creates a store and never indexes an unsafe root.
///
/// Precedence is first-matching-tier, not a union: once a tier yields one or
/// more store-bearing roots, later tiers are ignored. Cursor multi-root
/// post-tool-use therefore rebuilds only the repo that owns `file_path`, not
/// every entry in `workspace_roots`.
pub fn select_roots(payload: &Value, pinned_root: Option<&Path>) -> anyhow::Result<RootSelection> {
    if let Some(pin) = pinned_root {
        let root = canonicalize_existing(pin)?;
        if is_unsafe_root(&root) {
            bail!(
                "refusing pinned root {}: $HOME, `/`, or a temp directory",
                root.display()
            );
        }
        if store_exists(&root) {
            return Ok(RootSelection::whole(vec![root]));
        }
        return Ok(RootSelection::whole(Vec::new()));
    }

    // A bound that bit in a tier that then found nothing is still a bound:
    // the sweep stopped early, so a later tier's answer is a lower bound too.
    let mut capped = false;

    // Tier 1: tool_input.file_path / file_path
    if let Some(path) = file_path_from_payload(payload) {
        let (selected, tier_capped) = store_roots_from([path]);
        capped |= tier_capped;
        if !selected.is_empty() {
            return Ok(RootSelection {
                roots: selected,
                capped,
            });
        }
    }

    // Tier 2: Codex apply_patch paths in tool_input.command
    let patch_paths = apply_patch_paths(payload);
    if !patch_paths.is_empty() {
        let (selected, tier_capped) = store_roots_from(patch_paths);
        capped |= tier_capped;
        if !selected.is_empty() {
            return Ok(RootSelection {
                roots: selected,
                capped,
            });
        }
    }

    // Tier 3: cwd worktree
    if let Some(cwd) = string_path(payload, "cwd") {
        let (selected, tier_capped) = store_roots_from([cwd]);
        capped |= tier_capped;
        if !selected.is_empty() {
            return Ok(RootSelection {
                roots: selected,
                capped,
            });
        }
    }

    // Tier 4: every workspace_roots[] entry (only when earlier tiers missed)
    if let Some(roots) = payload.get("workspace_roots").and_then(Value::as_array) {
        let candidates: Vec<PathBuf> = roots
            .iter()
            .filter_map(|root| root.as_str().map(PathBuf::from))
            .collect();
        if !candidates.is_empty() {
            let (selected, tier_capped) = store_roots_from(candidates);
            capped |= tier_capped;
            if !selected.is_empty() {
                return Ok(RootSelection {
                    roots: selected,
                    capped,
                });
            }
        }
    }

    // Tier 5: CLAUDE_PROJECT_DIR / CURSOR_PROJECT_DIR
    let mut env_candidates = Vec::new();
    for env_key in ["CLAUDE_PROJECT_DIR", "CURSOR_PROJECT_DIR"] {
        if let Ok(value) = std::env::var(env_key) {
            if !value.is_empty() {
                push_unique(&mut env_candidates, Some(PathBuf::from(value)));
            }
        }
    }
    let (selected, tier_capped) = store_roots_from(env_candidates);
    Ok(RootSelection {
        roots: selected,
        capped: capped | tier_capped,
    })
}

/// The roots one hook will act on, and whether a bound stopped the search.
///
/// `capped` is carried rather than dropped so the hook can say it covered part
/// of the session: a selection that hit [`MAX_HOOK_ROOTS`] or
/// [`MAX_HOOK_ROOT_CANDIDATES`] is a lower bound, not the whole workspace.
#[derive(Debug, Clone, Default)]
pub struct RootSelection {
    pub roots: Vec<PathBuf>,
    pub capped: bool,
}

impl RootSelection {
    fn whole(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            capped: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

/// Resolve candidates to unique store-bearing, safe repository roots.
///
/// Every tier funnels through here, so both bounds live here: at most
/// [`MAX_HOOK_ROOT_CANDIDATES`] are examined and at most [`MAX_HOOK_ROOTS`]
/// are returned. Reaching either is reported by [`select_roots`] rather than
/// silently trimmed — a hook that acted on 8 of 800 repositories has not
/// covered the session.
fn store_roots_from<I>(candidates: I) -> (Vec<PathBuf>, bool)
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut selected = Vec::new();
    let mut capped = false;
    for (examined, candidate) in candidates.into_iter().enumerate() {
        if examined >= MAX_HOOK_ROOT_CANDIDATES {
            capped = true;
            break;
        }
        let Ok(root) = resolve_repo_root(&candidate) else {
            continue;
        };
        if is_unsafe_root(&root) {
            continue;
        }
        if store_exists(&root) {
            push_unique(&mut selected, Some(root));
            if selected.len() >= MAX_HOOK_ROOTS {
                capped = true;
                break;
            }
        }
    }
    (selected, capped)
}

fn file_path_from_payload(payload: &Value) -> Option<PathBuf> {
    payload
        .pointer("/tool_input/file_path")
        .or_else(|| payload.get("file_path"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// Codex `apply_patch` carries paths inside `tool_input.command`, not `file_path`.
fn apply_patch_paths(payload: &Value) -> Vec<PathBuf> {
    let Some(command) = payload
        .pointer("/tool_input/command")
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut paths = Vec::new();
    for line in command.lines() {
        let trimmed = line.trim();
        for prefix in ["*** Update File:", "*** Add File:", "*** Delete File:"] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let path = PathBuf::from(rest.trim());
                let absolute = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                paths.push(absolute);
            }
        }
    }
    paths
}

fn string_path(payload: &Value, key: &str) -> Option<PathBuf> {
    payload.get(key).and_then(Value::as_str).map(PathBuf::from)
}

fn push_unique(out: &mut Vec<PathBuf>, path: Option<PathBuf>) {
    let Some(path) = path else {
        return;
    };
    if out.iter().any(|existing| existing == &path) {
        return;
    }
    out.push(path);
}

fn resolve_repo_root(path: &Path) -> anyhow::Result<PathBuf> {
    let path = if path.is_file() {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    // Walk up until a store is found, else canonicalize the path itself.
    let mut cursor = canonicalize_existing(&path).unwrap_or(path);
    loop {
        if store_exists(&cursor) {
            return Ok(cursor);
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent.to_path_buf(),
            _ => break,
        }
    }
    canonicalize_existing(&cursor).or_else(|_| Ok(cursor))
}

fn canonicalize_existing(path: &Path) -> anyhow::Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))
}

fn store_exists(root: &Path) -> bool {
    // Clear DEVMAP_HOME influence for hook discovery: the store must live under
    // the repository itself. Using the path helpers with an empty home override
    // would require plumbing; check both layout names directly.
    root.join(".devmap")
        .join("codeintel")
        .join("devmap.sqlite")
        .is_file()
        || root
            .join(".devcouncil")
            .join("codeintel")
            .join("devmap.sqlite")
            .is_file()
}

fn is_unsafe_root(root: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut banned = vec![PathBuf::from("/"), std::env::temp_dir()];
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            banned.push(PathBuf::from(home));
        }
    }
    for extra in ["/tmp", "/private/tmp", "/var/tmp"] {
        banned.push(PathBuf::from(extra));
    }
    banned
        .iter()
        .any(|path| path.canonicalize().unwrap_or_else(|_| path.clone()) == root)
}

/// What the agent should do with the index, stated once per session.
///
/// SessionStart stdout is the one place every host pastes into the model's
/// context, so it is the only reliable moment to say what the index is for.
/// An index nothing queries costs build time and saves nothing: the tool
/// names are the adoption lever, and the caveat is what keeps an empty answer
/// from being read as proof of absence.
const DEVMAP_DIRECTIVE: &str = "Ask DevMap before reading files: devmap_search, devmap_explore, \
     devmap_impact, devmap_trace, devmap_affected_tests. Check truncated/walk_incomplete before \
     treating an empty list as \"does not exist\".";

/// Longest per-repository clause. Bounds one repository's contribution so a
/// pathological status payload cannot crowd out the directive or the notes.
const REPO_CLAUSE_CAP: usize = 200;

/// Longest installation-health *detail*. One per session, not one per
/// repository: a mismatched plugin bundle is a property of the installation.
///
/// The detail is what gets elided; [`HEALTH_REMEDY`] is appended afterwards so
/// the action always survives. The warning text concatenates absolute cache
/// paths, which are long and near-identical, so a flat truncation spent the
/// budget on the paths and cut the command that fixes it.
const HEALTH_NOTE_CAP: usize = 200;

/// What to do about any condition [`crate::plugin_warning`] reports.
///
/// Every one of them — a version that disagrees with the binary, a malformed
/// bundle layout, hooks still written in the `args`/`async` form — is repaired
/// by regenerating the bundle, so a single constant remedy is accurate rather
/// than a guess.
const HEALTH_REMEDY: &str = "re-run `devmap claude plugin` and reinstall the bundle";

/// Defensive ceiling on the whole SessionStart context.
///
/// The real bound is structural — at most [`MAX_HOOK_ROOTS`] clauses of at
/// most [`REPO_CLAUSE_CAP`] each, plus a fixed directive and one note — so
/// this is a backstop rather than a routine trimmer. Sizing it above that
/// structural maximum is the point: the previous flat `truncate(.., 400)` ran
/// after the notes were appended, so on a capped or timed-out pass it deleted
/// the very sentence that said the pass was partial.
const SESSION_START_CONTEXT_CAP: usize =
    MAX_HOOK_ROOTS * REPO_CLAUSE_CAP + HEALTH_NOTE_CAP + HEALTH_REMEDY.len() + 512;

/// Coverage buckets that mean information was lost.
///
/// `pattern_recovered` is deliberately absent: it records a file that had no
/// grammar but still yielded declarations, which is a partial success, not a
/// blind spot. Counting it would inflate the number the agent uses to decide
/// whether to trust an empty result.
const LOSSY_COVERAGE_BUCKETS: &[&str] = &[
    "call_blind",
    "discovery_refused",
    "import_blind",
    "not_parsed",
    "parse_failed",
];

/// Render `devmap status` output as one clause an agent can act on.
///
/// The status payload is JSON — compact with `--json`, pretty-printed without
/// it. Both parse here, so this summary is identical whichever spelling a host
/// hook happens to use. That matters because installed plugin bundles pin an
/// older hook command line: the compact form is what the agent reads either
/// way, instead of 3 KB of pretty JSON or a 160-character slice through the
/// middle of a key.
fn summarize_status(raw: &str) -> String {
    // Whole-document first (pretty-printed), then line-wise (compact, possibly
    // preceded by a diagnostic line).
    let parsed = serde_json::from_str::<Value>(raw.trim()).ok().or_else(|| {
        raw.lines()
            .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
    });
    let Some(status) = parsed else {
        // Never paste an unparsed blob. A mid-token JSON fragment costs the
        // same tokens as a sentence and carries none of the meaning.
        let detail = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if detail.is_empty() {
            return "status unavailable (no output)".into();
        }
        return format!("status unreadable: {}", truncate(&detail, 80));
    };

    let flag = |key: &str| status.get(key).and_then(Value::as_bool);
    let count = |key: &str| status.get(key).and_then(Value::as_u64);
    let text = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };

    // Readiness leads, because it is the only field that changes what the
    // agent does next: an index that cannot answer must not be queried as if
    // it could, and saying so is worth more than any count.
    if !flag("query_ready").unwrap_or(false) {
        let why = text("degraded_reason")
            .or_else(|| text("rebuild_reason"))
            .map(|reason| format!(" ({})", truncate(reason, 60)))
            .unwrap_or_default();
        return format!("NOT QUERYABLE{why} — run `devmap build --manifest`");
    }

    let rebuild_required = flag("rebuild_required").unwrap_or(false);
    let mut clause = String::from(if rebuild_required {
        "stale"
    } else if flag("is_fresh").unwrap_or(false) {
        "ready"
    } else {
        // Queryable, but the tree has moved since the generation was built.
        "ready (source moved)"
    });

    if let (Some(nodes), Some(edges)) = (count("node_count"), count("edge_count")) {
        clause.push_str(&format!(", {nodes} symbols/{edges} edges"));
    }
    if let Some(generation) = count("generation_id") {
        clause.push_str(&format!(" (gen {generation})"));
    }
    if rebuild_required {
        if let Some(reason) = text("rebuild_reason") {
            clause.push_str(&format!("; rebuild: {}", truncate(reason, 50)));
        }
    }
    if let Some(reason) = text("degraded_reason") {
        clause.push_str(&format!("; degraded: {}", truncate(reason, 50)));
    }
    let blind = lossy_coverage_total(&status);
    if blind > 0 {
        clause.push_str(&format!("; {blind} file(s) not fully parsed"));
    }
    match count("quarantined_count") {
        Some(quarantined) if quarantined > 0 => {
            clause.push_str(&format!("; {quarantined} quarantined"));
        }
        _ => {}
    }
    clause
}

/// Files whose content the index could not fully read, across every lossy
/// bucket. Each bucket reports its own `total`, which is the honest number:
/// `paths` is a capped sample.
fn lossy_coverage_total(status: &Value) -> u64 {
    let Some(gaps) = status.get("coverage_gaps").and_then(Value::as_object) else {
        return 0;
    };
    LOSSY_COVERAGE_BUCKETS
        .iter()
        .filter_map(|bucket| gaps.get(*bucket))
        .filter_map(|bucket| bucket.get("total").and_then(Value::as_u64))
        .sum()
}

/// The counts sentence from `session-report --last`, without its advice.
///
/// The report ends with guidance this hook already states once, in
/// [`DEVMAP_DIRECTIVE`]; repeating it per repository would spend the budget
/// saying the same thing several times. The counts are what vary, and a run of
/// zero queries is the signal worth carrying: it says the last session paid to
/// build an index it never asked anything.
fn summarize_last_session(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let sentence = line.split_once(". ").map_or(line, |(head, _)| head);
    let sentence = sentence
        .trim()
        .trim_end_matches('.')
        .trim_start_matches("DevMap session:")
        .trim();
    if sentence.is_empty() {
        return None;
    }
    Some(truncate(sentence, 90))
}

fn session_start_sync(executable: &Path, selection: &RootSelection) -> anyhow::Result<Value> {
    let started = Instant::now();
    let mut clauses: Vec<String> = Vec::new();
    let mut ran_out_of_time = false;
    for root in &selection.roots {
        let Some(status_text) = probe(
            executable,
            root,
            &["--json", "status", "--auto-rebuild"],
            started,
        )?
        else {
            ran_out_of_time = true;
            break;
        };
        let Some(last_text) = probe(executable, root, &["session-report", "--last"], started)?
        else {
            ran_out_of_time = true;
            break;
        };
        let mut clause = format!(
            "{}: {}",
            root.file_name().and_then(|n| n.to_str()).unwrap_or("repo"),
            summarize_status(&status_text)
        );
        if let Some(last) = summarize_last_session(&last_text) {
            clause.push_str(&format!("; last session {last}"));
        }
        clauses.push(truncate(&clause, REPO_CLAUSE_CAP));
    }

    let additional = compose_session_start(
        clauses,
        ran_out_of_time,
        selection.capped,
        selection.roots.len(),
        crate::plugin_warning(),
    );
    Ok(json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": additional,
        }
    }))
}

/// Assemble the context block from already-summarized repository clauses.
///
/// Separate from [`session_start_sync`] because the defect this replaced lived
/// here and nowhere else: the honesty note was appended and *then* a flat
/// `truncate(.., 400)` ran over the join, so precisely the pass that needed to
/// admit it was partial was the pass whose admission got cut. Budget logic
/// reachable only by spawning child processes is budget logic that never gets
/// a direct test, which is why this takes clauses rather than roots.
fn compose_session_start(
    clauses: Vec<String>,
    ran_out_of_time: bool,
    capped: bool,
    total_roots: usize,
    health: Option<String>,
) -> String {
    let mut lines = clauses;
    // Installation health, stated once. A bundle whose hooks still use
    // `args`/`async` runs the pre-native command line, which pastes the whole
    // `status` document into the context instead of the summary below — the
    // condition is already detected, and was reported only by `devmap paths`,
    // which nothing runs on its own.
    if let Some(note) = health {
        let note = note.trim();
        if !note.is_empty() {
            lines.push(format!(
                "DevMap install: {} — {HEALTH_REMEDY}",
                truncate(note, HEALTH_NOTE_CAP)
            ));
        }
    }
    if ran_out_of_time {
        lines.push(format!(
            "(stopped after {} of {} repo(s): {:?} budget)",
            lines.len(),
            total_roots,
            SESSION_START_BUDGET
        ));
    } else if capped {
        lines.push(format!(
            "(first {} repo(s); more were named than one hook acts on)",
            lines.len()
        ));
    }
    // The directive goes last so it is never the thing a backstop truncation
    // removes first, and is present even when no repository resolved.
    lines.push(DEVMAP_DIRECTIVE.into());

    // Bounded by construction: MAX_HOOK_ROOTS clauses of REPO_CLAUSE_CAP, one
    // note, one directive. The cap is a backstop for a future caller that
    // raises one of those without revisiting this.
    truncate(&lines.join("\n"), SESSION_START_CONTEXT_CAP)
}

/// One bounded child, or `None` when the SessionStart budget is already spent.
///
/// `run_bounded` is the kernel's own process boundary: it caps the wall clock,
/// kills the whole process group on expiry, and drains rather than buffers
/// output past the cap. A plain `output()` here had none of those — a child
/// that hung held the host's session-start open, and its stdout was buffered
/// whole before being truncated to one line.
fn probe(
    executable: &Path,
    root: &Path,
    args: &[&str],
    started: Instant,
) -> anyhow::Result<Option<String>> {
    let remaining = SESSION_START_BUDGET.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Ok(None);
    }
    let mut command = Command::new(executable);
    command.args(["--root"]).arg(root).args(args);
    let bounds = subprocess::Bounds {
        deadline: remaining,
        stdout_cap: CHILD_OUTPUT_CAP,
        stderr_cap: CHILD_OUTPUT_CAP,
    };
    match subprocess::run_bounded(&mut command, bounds) {
        Ok(captured) => Ok(Some(captured.stdout_lossy())),
        // The budget is the hook's contract with the host, not an error to
        // report: a slow repository yields a shorter summary, never a failure.
        Err(subprocess::Failure::Deadline { .. }) => Ok(None),
        Err(error) => Err(anyhow!(
            "{} for {}: {error}",
            args.join(" "),
            root.display()
        )),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Spawn one detached child per root, stopping when [`DETACH_BUDGET`] is spent.
///
/// Hosts block on this hook. Each root costs a lock acquisition, which touches
/// the filesystem, and one payload may name up to [`MAX_HOOK_ROOTS`] of them,
/// so the loop needs a deadline. It previously had a comment claiming one and
/// no enforcement: `let _ = started.elapsed() < DETACH_BUDGET;` computes the
/// comparison and discards it, so a slow filesystem blocked the host for as
/// long as every root took.
///
/// A truncated pass is never silent — the returned note names how many roots
/// went unindexed, because "no output" and "half the repositories were
/// skipped" must not look the same to the session that reads it.
fn detach_roots(
    executable: &Path,
    roots: &[PathBuf],
    budget: Duration,
    detach: fn(&Path, &Path) -> anyhow::Result<()>,
) -> anyhow::Result<Option<String>> {
    let started = Instant::now();
    for (index, root) in roots.iter().enumerate() {
        detach(executable, root)?;
        let remaining = roots.len() - index - 1;
        if remaining > 0 && started.elapsed() >= budget {
            return Ok(Some(format!(
                "devmap hook: {remaining} of {} repository/repositories left unindexed; \
                 the {} ms detach budget was spent",
                roots.len(),
                budget.as_millis()
            )));
        }
    }
    Ok(None)
}

/// One stderr line carries both notes, so neither hides the other.
fn join_notes(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

fn detach_build(executable: &Path, root: &Path) -> anyhow::Result<()> {
    let lock_dir = coalesce_lock_dir(root, "build");
    if !try_acquire_lock_dir(&lock_dir)? {
        return Ok(());
    }
    spawn_detached_with_cleanup(
        executable,
        &format!(
            "--root {} build",
            shell_single_quote(&root.display().to_string())
        ),
        &lock_dir,
    )
}

fn detach_session_report(executable: &Path, root: &Path) -> anyhow::Result<()> {
    let lock_dir = coalesce_lock_dir(root, "session-report");
    if !try_acquire_lock_dir(&lock_dir)? {
        return Ok(());
    }
    spawn_detached_with_cleanup(
        executable,
        &format!(
            "--root {} session-report",
            shell_single_quote(&root.display().to_string())
        ),
        &lock_dir,
    )
}

fn coalesce_lock_dir(root: &Path, kind: &str) -> PathBuf {
    let state = if root.join(".devmap").is_dir() {
        root.join(".devmap")
    } else {
        root.join(".devcouncil")
    };
    state.join("codeintel").join(format!("hook-{kind}.running"))
}

/// How many times one call may reclaim a stale lock before giving up.
///
/// Reclaiming is remove-then-create, and both halves can lose a race, so a
/// retry is legitimate. Recursing without a bound is not: a lock whose parent
/// directory is not writable reports `EEXIST` from `mkdir` and `EACCES` from
/// `rmdir` forever, and the previous code recursed on that identical state
/// until the thread overflowed its stack and aborted the process.
const LOCK_RECLAIM_ATTEMPTS: usize = 3;

/// How long a lock whose owner cannot be identified is still treated as held.
///
/// Taking a lock is two steps — create the directory, then write the pid — and
/// a concurrent hook can observe the gap. Treating an unreadable pid as proof
/// of abandonment let that hook delete a lock that had just been taken, so the
/// unknown case fails closed until the lock is at least this old.
const LOCK_PID_GRACE: Duration = Duration::from_secs(10);

/// Absolute ceiling on a held lock, regardless of what its pid file says.
///
/// pids are reused. "The recorded owner is running" is therefore not proof the
/// original owner still is, and without a ceiling one reused pid strands a
/// repository's builds permanently. A build that legitimately runs longer than
/// this is already past any budget this hook enforces.
const LOCK_MAX_AGE: Duration = Duration::from_secs(30 * 60);

fn try_acquire_lock_dir(lock_dir: &Path) -> anyhow::Result<bool> {
    if let Some(parent) = lock_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    for _ in 0..LOCK_RECLAIM_ATTEMPTS {
        match fs::create_dir(lock_dir) {
            Ok(()) => {
                // Provisional owner. `spawn_detached_with_cleanup` overwrites
                // this with the child's pid, because the child is what holds
                // and removes the lock.
                let _ = fs::write(lock_dir.join("pid"), std::process::id().to_string());
                return Ok(true);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !lock_dir_is_stale(lock_dir) {
                    return Ok(false);
                }
                // The failure decides, rather than being discarded: retrying
                // after a removal that did not happen re-enters on identical
                // state, which is the unbounded loop this replaced.
                fs::remove_dir_all(lock_dir)
                    .with_context(|| format!("reclaim stale lock {}", lock_dir.display()))?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("lock dir {}", lock_dir.display()))
            }
        }
    }
    // Every attempt lost the create race to another hook. That hook is doing
    // the work, which is what coalescing is for, so this is not an error.
    Ok(false)
}

/// Age of the lock, or `None` when it cannot be read.
///
/// `None` means "cannot tell", and every caller below treats that as *held*.
fn lock_dir_age(lock_dir: &Path) -> Option<Duration> {
    let modified = lock_dir.metadata().and_then(|meta| meta.modified()).ok()?;
    SystemTime::now().duration_since(modified).ok()
}

fn lock_dir_is_stale(lock_dir: &Path) -> bool {
    let age = lock_dir_age(lock_dir);
    if age.is_some_and(|age| age >= LOCK_MAX_AGE) {
        return true;
    }
    // An owner that cannot be identified is presumed live until the grace
    // window closes. `is_some_and` is what makes an unreadable *age* fail
    // closed too: nothing known about the lock keeps it held.
    let unknown_owner = || age.is_some_and(|age| age >= LOCK_PID_GRACE);
    let Ok(pid_text) = fs::read_to_string(lock_dir.join("pid")) else {
        return unknown_owner();
    };
    let Ok(pid) = pid_text.trim().parse::<i32>() else {
        return unknown_owner();
    };
    #[cfg(unix)]
    {
        // Signal 0: process existence check without delivery.
        let rc = unsafe { libc::kill(pid, 0) };
        rc != 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn shell_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn spawn_detached_with_cleanup(
    executable: &Path,
    args_shell: &str,
    lock_dir: &Path,
) -> anyhow::Result<()> {
    let exe = executable
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    let lock = lock_dir
        .to_str()
        .ok_or_else(|| anyhow!("lock path is not UTF-8"))?;
    // The lock directory is held for the child's lifetime so concurrent hooks
    // coalesce onto one build. The shell removes it when the child exits.
    let script = format!(
        "{} {}; status=$?; rm -rf {}; exit $status",
        shell_single_quote(exe),
        args_shell,
        shell_single_quote(lock)
    );
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn detached {}", executable.display()))?;
    // The lock is held for the CHILD's lifetime — the child's shell is what
    // removes it — so the child is the owner a staleness check must ask about.
    // The acquiring hook exits within milliseconds of this call, so recording
    // the hook's pid made every lock read as stale exactly when coalescing was
    // meant to begin, and each following hook started another concurrent build.
    match fs::write(lock_dir.join("pid"), child.id().to_string()) {
        Ok(()) => Ok(()),
        // A fast child can finish and remove the lock before this write lands.
        // A lock that is already gone means the work is done, not a failure.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("record lock owner in {}", lock_dir.display()))
        }
    }
}

/// Read all of stdin up to [`MAX_STDIN_BYTES`] + 1 (to detect overflow).
pub fn read_stdin_bounded() -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut handle = io::stdin().lock();
    let mut chunk = [0u8; 8192];
    loop {
        let n = handle.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len() + n > MAX_STDIN_BYTES + 1 {
            buf.extend_from_slice(&chunk[..n]);
            buf.truncate(MAX_STDIN_BYTES + 1);
            // Drain the rest so the writer does not get SIGPIPE mid-payload.
            let mut sink = [0u8; 8192];
            while handle.read(&mut sink)? > 0 {}
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

/// Absolute-path shell-form command for a hook handler.
#[allow(dead_code)]
pub fn shell_command(executable: &Path, event: HookEvent) -> anyhow::Result<String> {
    let abs = if executable.is_absolute() {
        executable.to_path_buf()
    } else {
        executable
            .canonicalize()
            .unwrap_or_else(|_| executable.to_path_buf())
    };
    let text = abs
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    Ok(format!("\"{text}\" hook {}", event.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    // ---- SessionStart context: what the agent actually reads ----------------
    //
    // Every test below asserts the *text pasted into the model's context*.
    // The hook's whole purpose is that text, and before these it was the one
    // thing nothing checked: the suite verified root selection and process
    // lifecycle while the summary shipped a 160-character slice through the
    // middle of a JSON key.

    /// A realistic compact `devmap --json status` payload.
    fn status_json() -> String {
        serde_json::json!({
            "query_ready": true,
            "is_fresh": true,
            "rebuild_required": false,
            "node_count": 10824,
            "edge_count": 35518,
            "generation_id": 2248,
            "degraded_reason": null,
            "rebuild_reason": null,
            "quarantined_count": 0,
            "coverage_gaps": {
                "import_blind": {"paths": [], "shown": 0, "total": 8, "truncated": false},
                "parse_failed": {"paths": [], "shown": 0, "total": 1, "truncated": false},
                "pattern_recovered": {"paths": [], "shown": 0, "total": 5, "truncated": false},
            },
        })
        .to_string()
    }

    #[test]
    fn status_summary_is_prose_not_a_json_fragment() {
        let summary = summarize_status(&status_json());
        // The defect this replaces: `.lines().next()` on single-line JSON took
        // the whole document, and truncate() then cut it mid-key.
        assert!(
            !summary.contains('{') && !summary.contains("\":"),
            "summary leaked raw JSON: {summary}"
        );
        assert!(summary.starts_with("ready"), "{summary}");
        assert!(summary.contains("10824 symbols/35518 edges"), "{summary}");
        assert!(summary.contains("gen 2248"), "{summary}");
    }

    #[test]
    fn pretty_and_compact_status_summarize_identically() {
        // A host hook pinned to an older command line runs `status` without
        // `--json` and gets pretty-printed JSON. Both spellings must reach the
        // agent as the same sentence, or the summary silently depends on which
        // plugin bundle happens to be installed.
        let compact = status_json();
        let pretty =
            serde_json::to_string_pretty(&serde_json::from_str::<Value>(&compact).unwrap())
                .unwrap();
        assert!(pretty.lines().count() > 5, "fixture is not multi-line");
        assert_eq!(summarize_status(&compact), summarize_status(&pretty));
    }

    #[test]
    fn lossy_coverage_excludes_pattern_recovered() {
        // 8 import_blind + 1 parse_failed = 9; the 5 pattern_recovered files
        // yielded declarations and are not blind spots.
        let summary = summarize_status(&status_json());
        assert!(summary.contains("9 file(s) not fully parsed"), "{summary}");
    }

    #[test]
    fn an_unqueryable_index_leads_with_that_and_names_the_fix() {
        let raw = serde_json::json!({
            "query_ready": false,
            "node_count": 0,
            "degraded_reason": "store missing",
        })
        .to_string();
        let summary = summarize_status(&raw);
        assert!(summary.starts_with("NOT QUERYABLE"), "{summary}");
        assert!(summary.contains("store missing"), "{summary}");
        assert!(summary.contains("devmap build --manifest"), "{summary}");
    }

    #[test]
    fn a_stale_index_says_so_and_why() {
        let raw = serde_json::json!({
            "query_ready": true,
            "is_fresh": false,
            "rebuild_required": true,
            "rebuild_reason": "schema behind",
            "node_count": 10,
            "edge_count": 20,
        })
        .to_string();
        let summary = summarize_status(&raw);
        assert!(summary.starts_with("stale"), "{summary}");
        assert!(summary.contains("schema behind"), "{summary}");
    }

    #[test]
    fn a_queryable_index_over_a_moved_tree_is_not_called_fresh() {
        let raw = serde_json::json!({
            "query_ready": true,
            "is_fresh": false,
            "rebuild_required": false,
            "node_count": 1,
            "edge_count": 2,
        })
        .to_string();
        assert!(summarize_status(&raw).starts_with("ready (source moved)"));
    }

    #[test]
    fn unparseable_status_never_pastes_a_raw_fragment() {
        // A panic message, a truncated write, a host that swallowed stdout.
        for raw in [
            "thread 'main' panicked at src/main.rs:1:1:\nboom",
            "{\"query_ready\": true, \"node_co",
            "<html>not json at all</html>",
        ] {
            let summary = summarize_status(raw);
            assert!(
                summary.starts_with("status unreadable:"),
                "{raw:?} -> {summary}"
            );
            assert!(summary.chars().count() <= 100, "unbounded: {summary}");
        }
        assert_eq!(
            summarize_status("   \n  "),
            "status unavailable (no output)"
        );
        assert_eq!(summarize_status(""), "status unavailable (no output)");
    }

    #[test]
    fn a_diagnostic_line_before_the_json_does_not_defeat_parsing() {
        let raw = format!("warning: rebuilding index\n{}", status_json());
        assert!(summarize_status(&raw).starts_with("ready"));
    }

    #[test]
    fn a_hostile_status_payload_stays_bounded() {
        // Every string the summary interpolates is attacker-influenced in the
        // sense that a corrupt store can produce it. None may unbound the line.
        let raw = serde_json::json!({
            "query_ready": true,
            "is_fresh": false,
            "rebuild_required": true,
            "rebuild_reason": "x".repeat(50_000),
            "degraded_reason": "y".repeat(50_000),
            "node_count": u64::MAX,
            "edge_count": u64::MAX,
            "generation_id": u64::MAX,
            "quarantined_count": u64::MAX,
        })
        .to_string();
        let summary = summarize_status(&raw);
        assert!(
            summary.chars().count() < 400,
            "len {}",
            summary.chars().count()
        );
        assert!(summary.contains('…'), "expected elision: {summary}");
    }

    #[test]
    fn last_session_keeps_the_counts_and_drops_the_repeated_advice() {
        let raw = "DevMap session: 0 queries, 0 truncated, 0 empty, 10 recorded gaps. \
                   Read truncated/walk_incomplete before treating an empty list as \
                   'does not exist'. Do not fall back to GitNexus.";
        let summary = summarize_last_session(raw).expect("a summary");
        assert!(summary.starts_with("0 queries"), "{summary}");
        assert!(!summary.contains("GitNexus"), "advice repeated: {summary}");
        assert!(summary.chars().count() <= 90);
        assert_eq!(summarize_last_session("   \n "), None);
    }

    #[test]
    fn the_directive_names_the_tools_and_survives_every_pass() {
        // Whatever else happens, the agent is told the index exists and how to
        // ask it. A session that resolves no repository still gets the tools.
        for (clauses, timed_out, capped) in [
            (vec![], false, false),
            (vec!["a: ready".to_string()], false, false),
            (vec!["a: ready".to_string()], true, false),
            (vec!["a: ready".to_string()], false, true),
        ] {
            let out = compose_session_start(clauses, timed_out, capped, 4, None);
            assert!(out.contains("devmap_search"), "{out}");
            assert!(out.contains("devmap_impact"), "{out}");
            assert!(out.contains("truncated/walk_incomplete"), "{out}");
        }
    }

    #[test]
    fn a_partial_pass_keeps_its_admission_under_a_full_budget() {
        // The regression this pins: notes were appended and then a flat
        // truncate(.., 400) ran over the join, so a full pass — exactly the
        // case that was partial — lost the sentence saying so.
        let clauses: Vec<String> = (0..MAX_HOOK_ROOTS)
            .map(|i| format!("repo{i}: {}", "x".repeat(REPO_CLAUSE_CAP)))
            .collect();

        let timed_out = compose_session_start(clauses.clone(), true, false, 64, None);
        assert!(
            timed_out.contains("stopped after"),
            "note lost: {timed_out}"
        );
        assert!(timed_out.contains("of 64 repo(s)"), "{timed_out}");
        assert!(timed_out.contains("devmap_search"), "directive lost");

        let capped = compose_session_start(clauses, false, true, 64, None);
        assert!(capped.contains("more were named"), "note lost: {capped}");
        assert!(capped.contains("devmap_search"), "directive lost");
    }

    #[test]
    fn a_stale_install_is_reported_where_the_session_can_see_it() {
        // The condition that motivated this: a plugin bundle pinned to an
        // older version keeps firing the pre-native hook command line, which
        // pastes the whole status document into context. Dev Map already
        // detects it; before this it said so only in `devmap paths`, which
        // nothing runs on its own.
        let out = compose_session_start(
            vec!["repo: ready".into()],
            false,
            false,
            1,
            Some("installed plugin version 0.1.1 does not match binary 0.2.1".into()),
        );
        assert!(out.contains("DevMap install:"), "{out}");
        assert!(out.contains("0.1.1"), "{out}");
        assert!(out.contains(HEALTH_REMEDY), "{out}");
        // The directive still survives alongside it.
        assert!(out.contains("devmap_search"), "{out}");
    }

    #[test]
    fn the_install_remedy_survives_an_over_long_detail() {
        // Real warnings concatenate absolute cache paths per installed
        // version, so the detail routinely exceeds its budget. Eliding it must
        // not elide the command that fixes the problem.
        let out = compose_session_start(
            vec!["repo: ready".into()],
            false,
            false,
            1,
            Some(format!("{} and more", "/very/long/cache/path".repeat(200))),
        );
        assert!(out.contains(HEALTH_REMEDY), "remedy elided: {out}");
        assert!(out.contains('…'), "expected the detail to be elided: {out}");
    }

    #[test]
    fn a_healthy_install_adds_no_line() {
        let out = compose_session_start(vec!["repo: ready".into()], false, false, 1, None);
        assert!(!out.contains("DevMap install:"), "{out}");
        // An empty-but-present warning is the same as none: no blank label.
        let blank = compose_session_start(
            vec!["repo: ready".into()],
            false,
            false,
            1,
            Some("   \n ".into()),
        );
        assert!(!blank.contains("DevMap install:"), "{blank}");
    }

    #[test]
    fn the_context_block_is_bounded_by_construction() {
        // Worst case the selector can produce: MAX_HOOK_ROOTS clauses, each
        // already capped at REPO_CLAUSE_CAP, plus a note and the directive.
        let clauses: Vec<String> = (0..MAX_HOOK_ROOTS)
            .map(|i| truncate(&format!("repo{i}: {}", "x".repeat(1_000)), REPO_CLAUSE_CAP))
            .collect();
        for clause in &clauses {
            assert!(clause.chars().count() <= REPO_CLAUSE_CAP);
        }
        let out = compose_session_start(
            clauses,
            true,
            false,
            MAX_HOOK_ROOTS,
            Some("x".repeat(5_000)),
        );
        assert!(
            out.chars().count() <= SESSION_START_CONTEXT_CAP,
            "len {} > cap {}",
            out.chars().count(),
            SESSION_START_CONTEXT_CAP
        );
        // The backstop must not be the thing doing the trimming in the worst
        // case the selector can actually reach, or the note is at risk again.
        assert!(
            !out.ends_with('…'),
            "backstop truncated a structural worst case"
        );
    }

    #[test]
    fn a_multibyte_status_reason_is_not_split_mid_character() {
        let raw = serde_json::json!({
            "query_ready": true,
            "is_fresh": false,
            "rebuild_required": true,
            "rebuild_reason": "日本語".repeat(200),
            "node_count": 1,
            "edge_count": 1,
        })
        .to_string();
        // truncate() counts chars; a byte-slicing implementation panics here.
        let summary = summarize_status(&raw);
        assert!(summary.contains("日本語"), "{summary}");
    }

    #[test]
    fn empty_and_malformed_stdin_are_noop_exit_zero() {
        let exe = PathBuf::from("/bin/true");
        let empty = run_hook(HookEvent::PostToolUse, b"", &exe, None);
        assert_eq!(empty.exit_code, 0);
        let bad = run_hook(HookEvent::PostToolUse, b"{not-json", &exe, None);
        assert_eq!(bad.exit_code, 0);
        assert!(bad.stderr_line.unwrap().contains("malformed"));
    }

    #[test]
    fn cursor_payload_selects_file_path_root_with_store() {
        let dir = std::env::temp_dir().join(format!(
            "devmap-hook-sel-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".devcouncil/codeintel")).unwrap();
        fs::write(dir.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        let file = dir.join("src/a.rs");
        fs::write(&file, "fn a() {}").unwrap();
        let payload = json!({
            "workspace_roots": [dir.to_string_lossy()],
            "tool_input": {"file_path": file.to_string_lossy()},
        });
        let roots = select_roots(&payload, None).unwrap().roots;
        assert_eq!(roots, vec![dir.canonicalize().unwrap()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_path_tier_does_not_union_workspace_roots() {
        // Stress-pass regression: Cursor multi-root stdin carries every open
        // workspace in workspace_roots. Selecting file_path must not also
        // detach-build the siblings.
        let stamp = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let base = std::env::temp_dir().join(format!("devmap-hook-union-{stamp}"));
        let manvi = base.join("Manvi");
        let other_a = base.join("DevCouncil");
        let other_b = base.join("GitPulse");
        let other_c = base.join("MarkDev");
        for root in [&manvi, &other_a, &other_b, &other_c] {
            let _ = fs::remove_dir_all(root);
            fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
            fs::write(root.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
        }
        fs::create_dir_all(manvi.join("src")).unwrap();
        let file = manvi.join("src/a.rs");
        fs::write(&file, "fn a() {}").unwrap();

        let payload = json!({
            "workspace_roots": [
                other_a.to_string_lossy(),
                other_b.to_string_lossy(),
                other_c.to_string_lossy(),
                manvi.to_string_lossy(),
            ],
            "tool_input": {"file_path": file.to_string_lossy()},
        });
        let roots = select_roots(&payload, None).unwrap().roots;
        assert_eq!(
            roots,
            vec![manvi.canonicalize().unwrap()],
            "file_path must win alone; old union selected all workspace_roots: {roots:?}"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn cwd_tier_does_not_union_workspace_roots() {
        let stamp = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let base = std::env::temp_dir().join(format!("devmap-hook-cwd-{stamp}"));
        let primary = base.join("primary");
        let sibling = base.join("sibling");
        for root in [&primary, &sibling] {
            fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
            fs::write(root.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
        }
        let payload = json!({
            "cwd": primary.to_string_lossy(),
            "workspace_roots": [
                sibling.to_string_lossy(),
                primary.to_string_lossy(),
            ],
        });
        let roots = select_roots(&payload, None).unwrap().roots;
        assert_eq!(roots, vec![primary.canonicalize().unwrap()]);
        let _ = fs::remove_dir_all(&base);
    }

    /// Distinct per call: a nanosecond stamp alone collides when these run in
    /// parallel on macOS.
    fn scratch(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "devmap-hook-{label}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn store_bearing(root: &Path) {
        fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
        fs::write(root.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
    }

    /// `workspace_roots[]` is an unbounded array from the payload, and a
    /// repository can carry as many nested store-bearing directories as it
    /// likes. Uncapped, each one cost two child processes on SessionStart.
    #[test]
    fn workspace_roots_fan_out_is_capped_and_says_so() {
        let base = scratch("fanout");
        let mut named = Vec::new();
        for i in 0..(MAX_HOOK_ROOTS * 4) {
            let root = base.join(format!("r{i}"));
            store_bearing(&root);
            named.push(root.to_string_lossy().into_owned());
        }
        let payload = json!({ "workspace_roots": named });

        let selection = select_roots(&payload, None).unwrap();
        assert_eq!(
            selection.roots.len(),
            MAX_HOOK_ROOTS,
            "acted on {} roots; the cap is {MAX_HOOK_ROOTS}",
            selection.roots.len()
        );
        assert!(
            selection.capped,
            "trimmed the selection without reporting it as partial"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// Candidates that resolve to nothing still cost a canonicalization each.
    #[test]
    fn unresolvable_candidate_sweep_is_capped() {
        let base = scratch("candidates");
        fs::create_dir_all(&base).unwrap();
        let named: Vec<String> = (0..(MAX_HOOK_ROOT_CANDIDATES * 2))
            .map(|i| {
                base.join(format!("absent{i}"))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let payload = json!({ "workspace_roots": named });

        let selection = select_roots(&payload, None).unwrap();
        assert!(selection.roots.is_empty());
        assert!(
            selection.capped,
            "swept every candidate without reporting the bound"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// An ordinary multi-root workspace under the cap is untouched by it.
    #[test]
    fn ordinary_workspace_is_not_capped() {
        let base = scratch("ordinary");
        let mut named = Vec::new();
        for i in 0..3 {
            let root = base.join(format!("r{i}"));
            store_bearing(&root);
            named.push(root.to_string_lossy().into_owned());
        }
        let payload = json!({ "workspace_roots": named });

        let selection = select_roots(&payload, None).unwrap();
        assert_eq!(selection.roots.len(), 3);
        assert!(!selection.capped, "reported a bound that never bit");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_patch_paths_are_resolved_against_cwd() {
        let payload = json!({
            "cwd": "/repo",
            "tool_input": {
                "command": "*** Begin Patch\n*** Update File: src/a.rs\n*** End Patch"
            }
        });
        let paths = apply_patch_paths(&payload);
        assert_eq!(paths, vec![PathBuf::from("/repo/src/a.rs")]);
    }

    #[test]
    fn unsafe_home_without_store_is_skipped() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(home) = home else {
            return;
        };
        let payload = json!({"cwd": home});
        let roots = select_roots(&payload, None).unwrap().roots;
        assert!(roots.is_empty(), "{roots:?}");
    }
}

#[cfg(all(test, unix))]
mod lock_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "devmap-lock-{label}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// A pid that is certainly dead: spawn a trivial child and reap it.
    fn reaped_pid() -> i32 {
        let mut child = Command::new("/usr/bin/true")
            .spawn()
            .or_else(|_| Command::new("/bin/true").spawn())
            .expect("spawn /bin/true");
        let pid = child.id() as i32;
        child.wait().expect("reap");
        pid
    }

    /// The lock must name the process that will REMOVE it.
    ///
    /// `try_acquire_lock_dir` records the acquiring process, but the acquirer
    /// is the hook, which exits within milliseconds of spawning the detached
    /// build. The build is what holds the lock, so recording the hook's pid
    /// makes every lock look stale to the next hook the moment the first one
    /// returns — which is exactly when coalescing is supposed to start.
    #[test]
    fn the_lock_names_the_process_that_outlives_the_hook() {
        let root = scratch("owner");
        let lock = root.join("lock");
        assert!(try_acquire_lock_dir(&lock).unwrap());

        spawn_detached_with_cleanup(Path::new("/bin/sleep"), "5", &lock).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let recorded: i32 = fs::read_to_string(lock.join("pid"))
            .expect("pid file")
            .trim()
            .parse()
            .expect("numeric pid");

        assert_ne!(
            recorded,
            std::process::id() as i32,
            "lock records the hook's pid; the hook exits immediately, so the \
             lock is stale as soon as it is taken"
        );
        assert!(
            alive(recorded),
            "recorded owner {recorded} is not running, so the next hook will \
             steal the lock and start a second concurrent build"
        );
        let _ = fs::remove_dir_all(&root);
    }

    // ---- detach budget ------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    // One counter per test. `detach` is a plain `fn` pointer so it cannot
    // capture, and a single shared static is racy under the parallel harness:
    // the two tests below interleaved and each read the other's increments.
    static SPENT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static UNSPENT_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn count_spent(_: &Path, _: &Path) -> anyhow::Result<()> {
        SPENT_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn count_unspent(_: &Path, _: &Path) -> anyhow::Result<()> {
        UNSPENT_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// A spent budget must stop the loop AND say what it skipped.
    ///
    /// The predecessor computed the comparison and threw it away, so a slow
    /// filesystem blocked the host for as long as every root took and the
    /// session was told nothing.
    #[test]
    fn a_spent_budget_stops_and_names_what_it_skipped() {
        let roots = vec![
            PathBuf::from("/one"),
            PathBuf::from("/two"),
            PathBuf::from("/three"),
        ];
        let note = detach_roots(Path::new("/bin/true"), &roots, Duration::ZERO, count_spent)
            .unwrap()
            .expect("a spent budget must report the roots it skipped");

        assert_eq!(
            SPENT_CALLS.load(Ordering::SeqCst),
            1,
            "the loop kept spawning after the budget was gone"
        );
        assert!(
            note.contains('2'),
            "note must count the skipped roots: {note}"
        );
    }

    /// The common case must be unchanged: every root indexed, nothing said.
    #[test]
    fn a_budget_that_is_not_spent_indexes_every_root() {
        let roots = vec![PathBuf::from("/one"), PathBuf::from("/two")];
        let note = detach_roots(
            Path::new("/bin/true"),
            &roots,
            Duration::from_secs(60),
            count_unspent,
        )
        .unwrap();
        assert_eq!(UNSPENT_CALLS.load(Ordering::SeqCst), 2);
        assert!(
            note.is_none(),
            "an unspent budget must stay silent: {note:?}"
        );
    }

    // ---- staleness fails closed ---------------------------------------

    /// Taking a lock is create-then-write. A hook observing the gap must not
    /// conclude the lock is free and delete it.
    #[test]
    fn a_lock_whose_pid_has_not_landed_yet_is_not_stolen() {
        let base = scratch("grace");
        let lock = base.join("hook-build.running");
        fs::create_dir_all(&lock).unwrap();
        assert!(
            !lock_dir_is_stale(&lock),
            "a just-created lock with no pid file yet was treated as abandoned"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// pids are reused, so a live pid cannot hold a lock open forever.
    #[test]
    fn a_lock_past_the_ceiling_is_reclaimed_whatever_its_pid_says() {
        let base = scratch("ceiling");
        let lock = base.join("hook-build.running");
        fs::create_dir_all(&lock).unwrap();
        // Our own pid: certainly alive, so only the age can make this stale.
        fs::write(lock.join("pid"), std::process::id().to_string()).unwrap();
        let status = Command::new("touch")
            .args(["-t", "200001010000"])
            .arg(&lock)
            .status()
            .expect("touch");
        assert!(status.success());

        assert!(
            lock_dir_is_stale(&lock),
            "a lock older than LOCK_MAX_AGE stayed held because its pid is alive"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// A stale lock that cannot be removed must fail, not recurse forever.
    ///
    /// `rmdir` needs write permission on the PARENT, so a read-only parent
    /// makes removal fail while `mkdir` still reports EEXIST. The retry then
    /// re-enters on identical state with no depth bound.
    #[test]
    fn an_unremovable_stale_lock_terminates() {
        let base = scratch("unremovable");
        let parent = base.join("codeintel");
        let lock = parent.join("hook-build.running");
        fs::create_dir_all(&lock).unwrap();
        fs::write(lock.join("pid"), reaped_pid().to_string()).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).unwrap();

        let outcome = try_acquire_lock_dir(&lock);

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let _ = fs::remove_dir_all(&base);

        match outcome {
            Err(_) => {}
            Ok(false) => {}
            Ok(true) => panic!("claimed a lock it could not create"),
        }
    }
}
