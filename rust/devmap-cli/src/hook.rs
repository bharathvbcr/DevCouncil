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

/// Total wall clock a synchronous PreToolUse may spend in child processes.
///
/// Two orders of magnitude tighter than [`SESSION_START_BUDGET`] because the
/// host blocks on this hook *inside* a tool call rather than once at startup.
/// Only the first navigation tool call of a session ever reaches a child at
/// all — see [`claim_once`] — so this budget is paid once, not per call.
pub const PRE_TOOL_USE_BUDGET: Duration = Duration::from_millis(1500);

/// How many per-session markers one repository keeps before the oldest are
/// dropped.
///
/// Markers are one empty file per session, so the cost is inodes rather than
/// bytes, but "small forever" is not a bound. Pruning keeps the newest and
/// discards the rest, which at worst re-nudges a session whose marker aged out
/// of a very busy repository — a duplicate line, not a wrong one.
pub const MAX_PRE_TOOL_MARKERS: usize = 64;

/// How many directory entries pruning will examine. A marker directory that
/// somehow grew past this is trimmed over several invocations instead of
/// stalling one tool call on an unbounded readdir.
pub const MAX_PRUNE_SCAN: usize = 512;

/// Longest session identifier accepted from the payload before it is folded to
/// a digest. Hosts send UUIDs; this bounds a hostile or pathological one.
const MAX_SESSION_ID_BYTES: usize = 512;

/// Longest prefix of a shell command inspected for a search verb. A command is
/// classified by how it starts, so reading further buys nothing.
const MAX_COMMAND_SNIFF_BYTES: usize = 256;

/// Tool names, lowercased, whose use means the agent is navigating source.
///
/// Host-neutral on purpose: the matcher in a host's config already narrows
/// this, but a hook that trusts the matcher alone fires on everything under a
/// host that has no matcher (Codex) or spells the tools differently (Cursor).
const NAVIGATION_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "search",
    "read_file",
    "grep_search",
    "file_search",
    "codebase_search",
    "searchfiles",
    "readfile",
];

/// Shell verbs that make a terminal call a file scan rather than a build step.
///
/// Deliberately only true search verbs. `ls`, `cat` and `head` appear in almost
/// every shell call, so including them would move the nudge from "the first
/// time the agent looks for code" to "the first Bash call", which is usually
/// `git status` and tells the agent nothing about navigation.
const NAVIGATION_COMMANDS: &[&str] =
    &["rg", "grep", "egrep", "fgrep", "ugrep", "ag", "ack", "find"];

/// Shell verbs that read a file's contents rather than searching for one.
///
/// Kept apart from [`NAVIGATION_COMMANDS`] because a search verb is navigation
/// whatever its target, while a read verb is navigation only when it is aimed at
/// source. The distinction is not hypothetical: over 160,960 Bash calls measured
/// on 2026-09-17, 25.8% ended in a bare `| tail -25` or `| head -60` closing a
/// build or test run. Treating the verb alone as navigation would fire the nudge
/// on the tail of every `npm test`, which is the same mistake as counting the
/// session's first `git status`.
///
/// `ls` is deliberately absent: it names a directory, not a file to read, and it
/// is the most common leading verb in a shell session that is not looking for
/// code at all.
const READ_COMMANDS: &[&str] = &[
    "cat", "head", "tail", "sed", "awk", "bat", "nl", "less", "more",
];

/// Shell tool names whose payload carries a command to sniff.
const SHELL_TOOLS: &[&str] = &["bash", "shell", "run_terminal_cmd", "terminal"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    PreToolUse,
    PostToolUse,
    SessionEnd,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session-start",
            Self::PreToolUse => "pre-tool-use",
            Self::PostToolUse => "post-tool-use",
            Self::SessionEnd => "session-end",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "session-start" => Some(Self::SessionStart),
            "pre-tool-use" => Some(Self::PreToolUse),
            "post-tool-use" => Some(Self::PostToolUse),
            "session-end" => Some(Self::SessionEnd),
            _ => None,
        }
    }
}

/// Whether this payload came from Cursor's native hook runner.
///
/// Cursor documents `cursor_version` on every event. Claude and Codex do not
/// send it. Event-name casing is not a signal: both hosts have used both
/// spellings.
pub fn is_cursor_payload(payload: &Value) -> bool {
    payload
        .get("cursor_version")
        .is_some_and(|value| !value.is_null())
}

/// Cursor events whose stdout is a permission decision.
///
/// Invalid JSON or a schema mismatch on these **blocks the tool**. Dev Map
/// never installs on them; if a user wires the binary there anyway, we emit
/// nothing so the call fails open rather than blocking a Read.
fn is_cursor_permission_event(payload: &Value) -> bool {
    const EVENTS: &[&str] = &[
        "pretooluse",
        "beforereadfile",
        "beforeshellexecution",
        "beforemcpexecution",
        "beforetabfileread",
        "subagentstart",
    ];
    payload_str(payload, &["hook_event_name", "hookEventName"])
        .map(|name| name.trim().to_ascii_lowercase())
        .is_some_and(|name| EVENTS.contains(&name.as_str()))
}

/// Host-specific stdout for one hook result.
///
/// Cursor native `sessionStart` / `postToolUse` require JSON with snake_case
/// `additional_context`. Claude pastes exit-0 stdout into the model, so the
/// briefing must stay plaintext. `--json` bypasses this and emits the internal
/// document. Returns `None` when the host should see an empty body.
pub fn render_host_stdout(event: HookEvent, payload: &Value, stdout: &Value) -> Option<String> {
    let additional = stdout
        .pointer("/hookSpecificOutput/additionalContext")
        .and_then(Value::as_str);

    if is_cursor_payload(payload) {
        if is_cursor_permission_event(payload) {
            return None;
        }
        return match event {
            HookEvent::SessionStart | HookEvent::PreToolUse => {
                Some(json!({ "additional_context": additional.unwrap_or("") }).to_string())
            }
            HookEvent::PostToolUse | HookEvent::SessionEnd => {
                additional.map(|ctx| json!({ "additional_context": ctx }).to_string())
            }
        };
    }
    if let Some(ctx) = additional {
        Some(ctx.to_string())
    } else {
        Some(stdout.to_string())
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

    // Cheapest possible rejection, taken before `select_roots` does any
    // filesystem work. PreToolUse fires on every matching tool call for the
    // life of the session, so the overwhelmingly common outcome — "not a
    // navigation tool" — must cost a string compare and nothing else.
    if event == HookEvent::PreToolUse && !is_navigation_payload(&payload) {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: None,
            roots: Vec::new(),
        });
    }

    let selection = select_roots(&payload, pinned_root)?;
    if selection.is_empty() {
        // SessionStart only. Bounding the walk at the working tree turned a
        // false "ready" into silence, and silence is its own failure: the
        // agent cannot tell "DevMap has nothing to say" from "DevMap is not
        // here", so it reads an empty `devmap_search` as proof a symbol does
        // not exist. Name the tree and the one command that fixes it.
        //
        // Deliberately not on PreToolUse: that event decides an authorization
        // outcome, its handler is contracted to stay silent when the index
        // cannot answer, and `pre_tool_use_never_emits_a_permission_decision`
        // guards what it may emit. This says nothing there.
        let stdout = match event {
            HookEvent::SessionStart => unindexed_worktree_notice(&payload),
            _ => None,
        };
        return Ok(HookOutcome {
            exit_code: 0,
            stdout,
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
        HookEvent::PreToolUse => {
            let stdout = pre_tool_use_sync(executable, &selection, &payload)?;
            Ok(HookOutcome {
                exit_code: 0,
                stdout,
                // No `capped_note`: this hook acts on one root by construction,
                // so "the payload named more" is the normal case, not a partial
                // result worth reporting on every tool call.
                stderr_line: None,
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

/// What to say when this working tree is a repository with no index.
///
/// Only that case. A directory outside git, an unsafe root, or a tree that
/// does have a store are all `None`: there is either nothing true to say or
/// another branch already says it. The caveat at the end is the load-bearing
/// half — without it an agent reads an empty answer from a dead index as
/// proof of absence, which is the same confusion the false "ready" caused,
/// arrived at from the other side.
fn unindexed_worktree_notice(payload: &Value) -> Option<Value> {
    let cwd = canonicalize_existing(&string_path(payload, "cwd")?).ok()?;
    let root = devmap_extract::git_worktree_root(&cwd)?;
    if is_unsafe_root(&root) || store_exists(&root) {
        return None;
    }
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let text = format!(
        "{name}: no DevMap index in this working tree. Run `devmap build` here to enable \
         devmap_* queries; until then they cannot answer, and an empty result is not \
         evidence that a symbol is absent."
    );
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": text,
        }
    }))
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
    // ...but never out of this working tree. A linked worktree nested inside
    // its parent checkout (`.claude/worktrees/<lane>`) has no store until it
    // is built locally, and the parent's store describes a different commit.
    // Without this bound the walk reaches the parent and the session brief
    // reports that index as this session's: `status` says `query_ready:false`
    // while the hook says "ready, 11789 symbols", which is the one thing a
    // check that could not run must never do. `state_discovery_contract`
    // already holds the CLI to the same rule
    // (`a_new_linked_worktree_is_truthfully_missing_until_bootstrapped_locally`);
    // this is the hook honouring it. Outside git there is no repository
    // identity to cross, so discovery there is left unbounded.
    let boundary = devmap_extract::git_worktree_root(&cursor);
    loop {
        if store_exists(&cursor) {
            return Ok(cursor);
        }
        if boundary.as_deref() == Some(cursor.as_path()) {
            break;
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
            SESSION_START_BUDGET,
        )?
        else {
            ran_out_of_time = true;
            break;
        };
        let Some(last_text) = probe(
            executable,
            root,
            &["session-report", "--last"],
            started,
            SESSION_START_BUDGET,
        )?
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

/// Longest PreToolUse context line. Smaller than a SessionStart summary on
/// purpose: this one lands mid-tool-call, and a paragraph there costs the
/// agent more attention than it returns.
const PRE_TOOL_CONTEXT_CAP: usize = 400;

/// Restate the directive at the moment the agent is about to bypass the index.
///
/// SessionStart already carries [`DEVMAP_DIRECTIVE`], and the comment there
/// calls that "the only reliable moment". Measurement says it is reliable but
/// not sufficient: across the 37 session reports this repository had recorded
/// by 2026-09-14, 36 showed `query_count: 0` — every one of them had been told
/// about the index at startup and none of them asked it anything. A directive
/// competes with the whole session for attention; a directive attached to the
/// first `Read` competes with nothing.
///
/// Fires at most once per session per repository, spends at most one
/// subprocess doing it, and says nothing at all unless the index can actually
/// answer. Never blocks: the caller maps every outcome to exit 0.
fn pre_tool_use_sync(
    executable: &Path,
    selection: &RootSelection,
    payload: &Value,
) -> anyhow::Result<Option<Value>> {
    // One root by construction. The directive is about the index the agent is
    // about to bypass; probing eight repositories to say it once would put
    // eight subprocesses inside a single `Read`.
    let Some(root) = selection.roots.first() else {
        return Ok(None);
    };
    if claim_once(&marker_dir(root), &session_marker_name(payload)) == Claim::Skip {
        return Ok(None);
    }

    let started = Instant::now();
    // No `--auto-rebuild` here, unlike SessionStart: a rebuild triggered by a
    // `Read` is a surprise the agent did not ask for, and the cost lands inside
    // a tool call the host is blocking on.
    let Some(status_text) = probe(
        executable,
        root,
        &["--json", "status"],
        started,
        PRE_TOOL_USE_BUDGET,
    )?
    else {
        return Ok(None);
    };
    let Some(context) = compose_pre_tool_use(&status_text, root) else {
        return Ok(None);
    };
    Ok(Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": context,
        }
    })))
}

/// The directive, or nothing when the index cannot back it.
///
/// Silence is the correct output for an unbuilt, empty or degraded index.
/// Pointing an agent at `devmap_search` when the store cannot answer earns one
/// empty result and a durable conclusion that the tool does not work — which
/// is not hypothetical here: a sibling MCP server pinned to an older store
/// schema answers `available: false`, and the sessions that read it stopped
/// asking. A hook that oversells the index does more damage than one that
/// stays quiet.
fn compose_pre_tool_use(status_text: &str, root: &Path) -> Option<String> {
    let status: Value = serde_json::from_str(status_text).ok()?;
    if !status
        .get("query_ready")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    if status
        .get("degraded_reason")
        .is_some_and(|reason| !reason.is_null())
    {
        return None;
    }
    let nodes = status
        .get("node_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // A store that is query-ready but holds nothing answers every question
    // "absent", which is the single most expensive wrong answer it can give.
    if nodes == 0 {
        return None;
    }
    let edges = status
        .get("edge_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let generation = status
        .get("generation_id")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let fresh = status
        .get("is_fresh")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("this repository");
    let freshness = if fresh {
        "current"
    } else {
        "behind the working tree, so its answers are a lower bound"
    };
    Some(truncate(
        &format!(
            "DevMap has {name} indexed: {nodes} symbols, {edges} edges, \
             generation {generation}, {freshness}. {DEVMAP_DIRECTIVE}"
        ),
        PRE_TOOL_CONTEXT_CAP,
    ))
}

/// Whether this payload describes the agent about to look through source.
///
/// Two shapes count: a navigation tool by name, and a shell tool whose command
/// runs a search verb. Everything else — writes, edits, fetches, MCP calls, and
/// the `git status` that opens most sessions — is not a moment where the index
/// has anything to add.
fn is_navigation_payload(payload: &Value) -> bool {
    let Some(tool) = payload_str(payload, &["tool_name", "toolName", "tool"]) else {
        return false;
    };
    let tool = tool.trim().to_ascii_lowercase();
    if NAVIGATION_TOOLS.contains(&tool.as_str()) {
        return true;
    }
    if !SHELL_TOOLS.contains(&tool.as_str()) {
        return false;
    }
    payload
        .pointer("/tool_input/command")
        .or_else(|| payload.pointer("/toolInput/command"))
        .or_else(|| payload.pointer("/tool_input/cmd"))
        .or_else(|| payload.pointer("/command"))
        .and_then(Value::as_str)
        .is_some_and(command_is_search)
}

/// First string present under any of `keys`, at the payload's top level.
fn payload_str<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| payload.get(*key).and_then(Value::as_str))
}

/// Whether a shell command line runs a search verb.
///
/// Splits on pipeline and list separators because `cat x | rg y` is still a
/// scan, skips leading `VAR=value` assignments, and compares the verb's file
/// name so `/usr/bin/grep` matches `grep`. Only the first
/// [`MAX_COMMAND_SNIFF_BYTES`] are read: a command is classified by how it
/// starts, so a megabyte-long heredoc buys nothing.
fn command_is_search(command: &str) -> bool {
    let head: String = command
        .chars()
        .take(MAX_COMMAND_SNIFF_BYTES)
        .collect::<String>()
        .to_ascii_lowercase();
    head.split(['|', ';', '&', '(', ')', '\n']).any(|segment| {
        let Some(verb) = segment.split_whitespace().find(|word| !word.contains('=')) else {
            return false;
        };
        let verb = verb.rsplit('/').next().unwrap_or(verb);
        if NAVIGATION_COMMANDS.contains(&verb) {
            return true;
        }
        // A read verb is navigation only when *this* segment names source. The
        // segment, not the whole line: `npm test foo.rs | tail -25` must not be
        // navigation because of a filename that belongs to another segment.
        READ_COMMANDS.contains(&verb) && segment_reads_source(segment)
    })
}

/// Whether a shell segment names a file some language spec claims.
///
/// The extension table is [`devmap_extract::languages`], the same one the
/// indexer uses to decide what it will parse. A hand-kept list here would answer
/// "is this code" differently from the thing that indexes code, and the first
/// language added to one and not the other would make the nudge silently
/// language-specific.
///
/// A redirection target is skipped: `sed -n 1,5p x.rs > out.rs` is still a read
/// of `x.rs`, but `cat /dev/null > main.rs` names source only as a destination
/// and is not navigation.
fn segment_reads_source(segment: &str) -> bool {
    // Everything from the first redirection onward names a destination, not
    // something being read. Truncating there is simpler than tracking which
    // operator takes a following word, and it cannot mistake a write target for
    // a read: `cat /dev/null > main.rs` reads no source.
    let read_part = segment
        .split_once('>')
        .map_or(segment, |(before, _)| before);
    let mut words = read_part.split_whitespace();
    let _ = words.next(); // the verb
    words.any(|word| {
        let stem = word.trim_matches(|c: char| c == '"' || c == '\'' || c == ';');
        if stem.starts_with('-') {
            return false;
        }
        stem.rsplit_once('.').is_some_and(|(name, ext)| {
            !name.is_empty()
                && !ext.is_empty()
                && !ext.contains('/')
                && devmap_extract::languages::find_spec_by_extension(ext).is_some()
        })
    })
}

/// Where a repository keeps hook bookkeeping.
///
/// Shared with [`coalesce_lock_dir`] so the two never disagree about which
/// layout a repository uses; the choice is a property of the repository.
fn state_dir(root: &Path) -> PathBuf {
    if root.join(".devmap").is_dir() {
        root.join(".devmap")
    } else {
        root.join(".devcouncil")
    }
}

fn marker_dir(root: &Path) -> PathBuf {
    state_dir(root).join("codeintel").join("hooks")
}

/// Prefix every marker carries. Pruning removes only names that start with it,
/// so a marker directory shared with anything else cannot lose the other thing.
const MARKER_PREFIX: &str = "nav.";

/// Filesystem-safe, collision-free marker name for this session.
///
/// The readable prefix keeps the directory debuggable by eye; the FNV-1a suffix
/// is what makes two sessions sharing a 48-character prefix distinct, so a
/// truncated name can never silence a different session. A payload with no
/// session identifier falls back to the process id, which nudges once per hook
/// process — wrong in the harmless direction, where a host that omits the field
/// gets a few extra lines rather than none at all.
fn session_marker_name(payload: &Value) -> String {
    let raw = payload_str(
        payload,
        &[
            "conversation_id",
            "conversationId",
            "session_id",
            "sessionId",
            "session",
        ],
    )
    .map(str::trim)
    .filter(|id| !id.is_empty())
    .map(|id| id.chars().take(MAX_SESSION_ID_BYTES).collect::<String>())
    .unwrap_or_else(|| format!("anon-{}", std::process::id()));

    let digest = fnv1a64(raw.as_bytes());
    let readable: String = raw
        .chars()
        .take(48)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!("{MARKER_PREFIX}{readable}.{digest:016x}")
}

/// FNV-1a, written out rather than taken from `DefaultHasher`.
///
/// The marker has to mean the same thing across separate processes, and
/// `DefaultHasher`'s output is explicitly not guaranteed stable across Rust
/// releases. A rebuilt binary that hashed a session differently would re-nudge
/// every live session once — small, but silently version-dependent.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Outcome of trying to be the first navigation hook of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// This call created the marker and owes the session a directive.
    First,
    /// Another call already claimed it, or the filesystem refused. Either way
    /// this call says nothing: a duplicate directive costs context, and a hook
    /// that cannot write must never cost the agent its tool call.
    Skip,
}

/// Claim the session's one directive, atomically.
///
/// `create_new` is the whole mechanism: it is a single atomic syscall, so
/// exactly one member of a concurrent burst wins. A parallel fan-out of reads
/// emits one directive rather than one per call, without a lock to reclaim or
/// a pid to interpret.
fn claim_once(dir: &Path, name: &str) -> Claim {
    if fs::create_dir_all(dir).is_err() {
        return Claim::Skip;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(name))
    {
        Ok(_) => {
            prune_markers(dir, MAX_PRE_TOOL_MARKERS);
            Claim::First
        }
        Err(_) => Claim::Skip,
    }
}

/// Keep the newest `cap` markers and drop the rest.
///
/// Bounded twice: at most [`MAX_PRUNE_SCAN`] entries are examined, so a
/// directory that somehow grew far past the cap is trimmed over several
/// invocations instead of stalling one tool call on an unbounded readdir; and
/// only names carrying [`MARKER_PREFIX`] are eligible, so nothing else that
/// shares the directory can be removed. Every error is swallowed — pruning is
/// housekeeping, and housekeeping must never fail a tool call.
fn prune_markers(dir: &Path, cap: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut markers: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.take(MAX_PRUNE_SCAN).flatten() {
        let path = entry.path();
        let is_marker = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(MARKER_PREFIX));
        if !is_marker {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        markers.push((modified, path));
    }
    if markers.len() <= cap {
        return;
    }
    markers.sort_by_key(|left| std::cmp::Reverse(left.0));
    for (_, path) in markers.into_iter().skip(cap) {
        let _ = fs::remove_file(path);
    }
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
    budget: Duration,
) -> anyhow::Result<Option<String>> {
    let remaining = budget.saturating_sub(started.elapsed());
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
    state_dir(root)
        .join("codeintel")
        .join(format!("hook-{kind}.running"))
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

/// Ceiling on the lock's `pid` file. A pid is at most ten digits and a
/// newline; anything larger is not one, and reading it is pure cost.
const MAX_PID_BYTES: u64 = 64;

fn lock_dir_is_stale(lock_dir: &Path) -> bool {
    let age = lock_dir_age(lock_dir);
    if age.is_some_and(|age| age >= LOCK_MAX_AGE) {
        return true;
    }
    // An owner that cannot be identified is presumed live until the grace
    // window closes. `is_some_and` is what makes an unreadable *age* fail
    // closed too: nothing known about the lock keeps it held.
    let unknown_owner = || age.is_some_and(|age| age >= LOCK_PID_GRACE);
    // Bounded, and only from a regular file. A plain `read_to_string` here
    // pulled in whatever the path held: a pid file something had grown read
    // into memory whole, and a *fifo* left in its place blocked this call
    // forever — a hook that never returns, on the one path that must always
    // return. A pid is at most ten digits and a newline.
    let Some(pid_text) =
        devmap_query::stat_memo::read_bounded(&lock_dir.join("pid"), MAX_PID_BYTES)
    else {
        return unknown_owner();
    };
    let Ok(pid) = pid_text.trim().parse::<i32>() else {
        return unknown_owner();
    };
    // Only a strictly positive pid names a process. `kill` reads `0` as the
    // caller's own process group and a negative value as the group `-pid`, and
    // both answer SUCCESS — so a truncated or half-written pid file saying `0`
    // made the owner look permanently alive and stopped that repository's
    // rebuilds until LOCK_MAX_AGE expired half an hour later.
    if pid <= 0 {
        return unknown_owner();
    }
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
    use std::thread;
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

    /// A fake `devmap` that prints `stdout_json` for any arguments.
    ///
    /// The hook shells out to itself for status; a stub lets these tests drive
    /// the real emission path without building an index. Gated with its callers
    /// so a Windows build does not carry an unused POSIX-script helper.
    #[cfg(unix)]
    fn fake_devmap(dir: &Path, stdout_json: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let script = dir.join("fake-devmap.sh");
        fs::write(
            &script,
            format!("#!/bin/sh\ncat <<'EOF'\n{stdout_json}\nEOF\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    fn healthy_status() -> &'static str {
        r#"{"query_ready":true,"degraded_reason":null,"node_count":11714,
            "edge_count":35951,"generation_id":2582,"is_fresh":true}"#
    }

    /// Recursively collect every object key in a JSON document.
    #[cfg(unix)]
    fn all_keys(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    into.push(key.clone());
                    all_keys(child, into);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| all_keys(item, into)),
            _ => {}
        }
    }

    /// The security invariant, asserted against real emitted output.
    ///
    /// `claude::PERMISSION_DECIDING_EVENTS` keeps Dev Map's *emitted hook table*
    /// off PreToolUse, and `hooks_block` bails if that is ever edited. This
    /// covers the other half: the handler a user wires up themselves must not be
    /// able to influence an authorization outcome either. A code index has
    /// nothing to contribute to a permission decision, so the only key this
    /// event may carry is `additionalContext`.
    ///
    /// Falsifiable by construction: it scans the document the hook actually
    /// produced rather than restating the literal the emitter uses, so adding a
    /// decision field anywhere in the payload turns it red.
    ///
    /// Unix-only because the stub is a `#!/bin/sh` script. The property it
    /// guards is not platform-specific, and the conformance suite covers the
    /// same event everywhere; gating this keeps a Windows runner from failing
    /// on the fixture rather than on the behaviour.
    #[cfg(unix)]
    #[test]
    fn pre_tool_use_never_emits_a_permission_decision() {
        let root = scratch("pretool-nopermission");
        store_bearing(&root);
        let exe = fake_devmap(&root.join("bin"), healthy_status());

        let payload = json!({
            "session_id": "sec-1",
            "cwd": root.to_string_lossy(),
            "tool_name": "Read",
            "tool_input": {"file_path": root.join("a.rs").to_string_lossy()},
        });
        let outcome = run_hook(
            HookEvent::PreToolUse,
            serde_json::to_string(&payload).unwrap().as_bytes(),
            &exe,
            None,
        );
        assert_eq!(outcome.exit_code, 0, "{:?}", outcome.stderr_line);
        let stdout = outcome
            .stdout
            .expect("a healthy index emits the directive; the rest of this test needs it");

        let mut keys = Vec::new();
        all_keys(&stdout, &mut keys);
        for forbidden in [
            "permissionDecision",
            "permissionDecisionReason",
            "permission",
            "decision",
            "continue",
            "stopReason",
            "suppressOutput",
            "systemMessage",
        ] {
            assert!(
                !keys.iter().any(|key| key == forbidden),
                "PreToolUse emitted {forbidden:?}; this event decides authorization \
                 outcomes and a code index must not reach one. Keys: {keys:?}"
            );
        }
        assert_eq!(
            keys.iter()
                .filter(|key| *key == "additionalContext")
                .count(),
            1,
            "expected exactly one additionalContext, got keys {keys:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The common case must cost a string compare, not a filesystem walk.
    ///
    /// Asserted by behaviour rather than by timing: a payload naming a root that
    /// does not exist would make `select_roots` work and fail, so a non-
    /// navigation tool returning cleanly proves the short circuit ran first.
    #[test]
    fn pre_tool_use_ignores_non_navigation_tools() {
        let exe = PathBuf::from("/bin/true");
        for tool in ["Write", "Edit", "WebFetch", "TodoWrite", "NotebookEdit"] {
            let payload = json!({
                "session_id": "skip",
                "cwd": "/nonexistent/devmap/probe/root",
                "tool_name": tool,
            });
            let outcome = run_hook(
                HookEvent::PreToolUse,
                serde_json::to_string(&payload).unwrap().as_bytes(),
                &exe,
                None,
            );
            assert_eq!(outcome.exit_code, 0, "{tool}");
            assert!(outcome.stdout.is_none(), "{tool} produced stdout");
            assert!(
                outcome.stderr_line.is_none(),
                "{tool} produced a diagnostic"
            );
            assert!(outcome.roots.is_empty(), "{tool} resolved roots");
        }
    }

    /// A shell tool is navigation only when it actually searches.
    #[test]
    fn bash_counts_as_navigation_only_for_search_verbs() {
        for (command, expected) in [
            ("rg TODO src/", true),
            ("grep -rn foo .", true),
            ("/usr/bin/grep foo bar", true),
            ("RIPGREP_CONFIG_PATH= rg foo", true),
            ("cat notes.md | rg foo", true),
            ("find . -name '*.rs'", true),
            ("git status", false),
            ("cargo build --release", false),
            ("ls -la", false),
            ("echo grep", false),
        ] {
            let payload = json!({
                "tool_name": "Bash",
                "tool_input": {"command": command},
            });
            assert_eq!(
                is_navigation_payload(&payload),
                expected,
                "classifying {command:?}"
            );
        }
    }

    /// A read verb is navigation only when it is aimed at source.
    ///
    /// The false-accept half of this table is the point. Of 160,960 Bash calls
    /// measured on 2026-09-17, 25.8% ended in a bare `| tail -N` or `| head -N`
    /// closing a build or test run; classifying the verb alone would fire the
    /// nudge on the tail of every `npm test`. Each `false` row below is a shape
    /// that actually occurs in that corpus.
    #[test]
    fn a_read_verb_is_navigation_only_when_it_names_source() {
        for (command, expected) in [
            // Aimed at source: this is an agent reading code.
            ("sed -n '1,60p' scripts/run-vitest.js", true),
            ("cat rust/devmap-cli/src/hook.rs", true),
            ("head -40 main.go", true),
            ("sed -n '80,200p' manuscript_citation_verification.go", true),
            ("cat foo.rs 2>/dev/null", true),
            ("sed -n '1,5p' x.rs > /tmp/out", true),
            ("cat a.py | rg TODO", true),
            // Not source, or not a read of anything.
            ("npx vitest run scripts/__tests__/ 2>&1 | tail -25", false),
            ("cargo test 2>&1 | head -60", false),
            ("npm test -- scripts/ 2>&1 | tail -25", false),
            ("cat package.json", false),
            ("cat Cargo.lock", false),
            ("sed -n '155,200p' cloudbuild.yaml", false),
            ("cat .env", false),
            ("ls -la src/", false),
            ("git status", false),
            // A write target is not a read, even when it names source.
            ("cat /dev/null > main.rs", false),
            ("cat > lib.rs", false),
            // A bare read with no file argument names nothing to read.
            ("tail -25", false),
            ("cat", false),
        ] {
            let payload = json!({
                "tool_name": "Bash",
                "tool_input": {"command": command},
            });
            assert_eq!(
                is_navigation_payload(&payload),
                expected,
                "classifying {command:?}"
            );
        }
    }

    /// The extension table is the indexer's, not a second copy.
    ///
    /// A hand-kept list here would answer "is this code" differently from the
    /// thing that indexes code. This asserts the shared table is actually
    /// consulted, by checking a language no local list would have thought to
    /// include.
    #[test]
    fn source_detection_uses_the_shared_language_table() {
        // Languages no list written from memory here would have covered. The
        // first draft of this test asserted `.zig` and `.ex`, which the table
        // does not carry — the table is the authority, not recollection.
        for ext in [
            "rs", "go", "py", "ts", "swift", "kt", "scala", "dart", "vue", "nix", "sol",
        ] {
            let command = format!("cat thing.{ext}");
            assert!(
                command_is_search(&command),
                "`{command}` should be navigation; {ext} is in the language table"
            );
        }
        // Data, config and — most importantly — secrets. `.env` and `.lock`
        // appear elsewhere in `languages.rs`, in its ignore helpers rather than
        // in any spec's extensions, so the lookup must not claim them. Reading a
        // secrets file is never something to nudge an agent towards.
        for ext in [
            "json", "lock", "yaml", "yml", "toml", "md", "txt", "env", "csv",
        ] {
            let command = format!("cat thing.{ext}");
            assert!(
                !command_is_search(&command),
                "`{command}` must not be navigation; {ext} is not code"
            );
        }
        assert!(
            !command_is_search("cat .env"),
            "a bare secrets file has no name before its extension and is not source"
        );
    }

    /// A filename in one segment must not make another segment navigation.
    #[test]
    fn source_is_looked_for_in_the_reading_segment_only() {
        assert!(
            !command_is_search("cargo build lib.rs | tail -5"),
            "`tail -5` reads no file; the .rs belongs to the cargo segment"
        );
        assert!(
            command_is_search("cargo build | sed -n '1,5p' lib.rs"),
            "the sed segment does name source"
        );
    }

    /// `echo grep` must not count: the verb is `echo`, and only the first word
    /// of a segment is the verb.
    #[test]
    fn only_the_first_word_of_a_segment_is_the_verb() {
        let buried = format!("{} rg foo", "echo ".repeat(64));
        assert!(
            !command_is_search(&buried),
            "a search verb used as an argument is not a scan"
        );
    }

    /// The sniff bound is a real bound, not a comment.
    ///
    /// Needs a search verb that *starts a segment* past the cap, because a verb
    /// buried mid-segment is already rejected by the verb rule. An earlier
    /// version of this test used the mid-segment shape and so asserted nothing
    /// about the bound: removing `.take(MAX_COMMAND_SNIFF_BYTES)` left it green.
    #[test]
    fn command_sniffing_stops_at_the_bound() {
        let huge = format!("rg {}", "x".repeat(MAX_COMMAND_SNIFF_BYTES * 4));
        assert!(command_is_search(&huge), "a long argv still starts with rg");

        let filler = "echo x; ".repeat(MAX_COMMAND_SNIFF_BYTES);
        let past_bound = format!("{filler}rg foo");
        assert!(
            past_bound.len() > MAX_COMMAND_SNIFF_BYTES,
            "the fixture must actually exceed the bound"
        );
        assert!(
            !command_is_search(&past_bound),
            "a segment beginning past the sniff bound must not be read"
        );

        // The same shape inside the bound is found, so the test above is about
        // the bound rather than about segment splitting being broken.
        assert!(
            command_is_search("echo x; rg foo"),
            "a search verb starting a later segment within the bound is a scan"
        );
    }

    /// One directive per session, even when the reads arrive in parallel.
    ///
    /// `create_new` is the whole mechanism, so this is the test that would catch
    /// a future rewrite to read-then-write, which races.
    #[test]
    fn concurrent_first_reads_claim_exactly_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let root = scratch("pretool-race");
        store_bearing(&root);
        let dir = Arc::new(marker_dir(&root));
        let name = Arc::new(session_marker_name(&json!({"session_id": "race-1"})));
        let firsts = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..24 {
            let dir = Arc::clone(&dir);
            let name = Arc::clone(&name);
            let firsts = Arc::clone(&firsts);
            handles.push(thread::spawn(move || {
                if claim_once(&dir, &name) == Claim::First {
                    firsts.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            firsts.load(Ordering::Relaxed),
            1,
            "a 24-way burst claimed the session's one directive more than once"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// Two sessions are two directives; one session is one.
    #[test]
    fn claims_are_scoped_to_the_session() {
        let root = scratch("pretool-scope");
        store_bearing(&root);
        let dir = marker_dir(&root);

        let first = session_marker_name(&json!({"session_id": "alpha"}));
        let second = session_marker_name(&json!({"session_id": "beta"}));
        assert_eq!(claim_once(&dir, &first), Claim::First);
        assert_eq!(claim_once(&dir, &first), Claim::Skip);
        assert_eq!(claim_once(&dir, &second), Claim::First);
        assert_eq!(claim_once(&dir, &second), Claim::Skip);
        let _ = fs::remove_dir_all(&root);
    }

    /// Truncation must not silence a different session.
    #[test]
    fn long_session_ids_sharing_a_prefix_stay_distinct() {
        let shared = "s".repeat(120);
        let left = session_marker_name(&json!({"session_id": format!("{shared}-left")}));
        let right = session_marker_name(&json!({"session_id": format!("{shared}-right")}));
        assert_ne!(
            left, right,
            "two sessions sharing a 120-character prefix collapsed to one marker"
        );
        for name in [&left, &right] {
            assert!(name.starts_with(MARKER_PREFIX));
            assert!(
                !name.contains('/') && !name.contains(".."),
                "marker name is not a safe file name: {name}"
            );
        }
    }

    /// A payload with no session id still nudges, scoped to the process.
    #[test]
    fn missing_session_id_falls_back_to_process_scope() {
        let name = session_marker_name(&json!({"tool_name": "Read"}));
        assert!(
            name.contains(&format!("anon-{}", std::process::id())),
            "{name}"
        );
    }

    /// Cursor's common schema names the conversation `conversation_id`.
    /// Falling back to the process id would re-nudge on every Read, because
    /// each hook is a new process.
    #[test]
    fn conversation_id_identifies_the_session() {
        let named = session_marker_name(&json!({"conversation_id": "conv-stable"}));
        let again = session_marker_name(&json!({"conversation_id": "conv-stable"}));
        assert_eq!(named, again);
        assert!(
            !named.contains(&format!("anon-{}", std::process::id())),
            "conversation_id must not fall through to the process id: {named}"
        );
        let different = session_marker_name(&json!({"conversation_id": "conv-other"}));
        assert_ne!(named, different);
    }

    fn briefing_document() -> Value {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": "Ask DevMap before reading files: devmap_search.\nrepo \"quotes\"",
            }
        })
    }

    /// Cursor native sessionStart is JSON `additional_context`. A plaintext
    /// unwrap is the live 3.20 failure: "Failed to parse hook stdout as JSON".
    #[test]
    fn cursor_session_start_renders_json_additional_context() {
        let payload = json!({
            "cursor_version": "3.20.7",
            "hook_event_name": "sessionStart",
        });
        let rendered = render_host_stdout(HookEvent::SessionStart, &payload, &briefing_document())
            .expect("Cursor sessionStart must emit a body");
        let value: Value = serde_json::from_str(&rendered).expect("parseable JSON");
        let ctx = value["additional_context"].as_str().expect("context");
        assert!(ctx.contains("Ask DevMap"), "{ctx}");
        assert!(ctx.contains("quotes"), "quotes must survive JSON encoding");
        assert!(value.get("permission").is_none());
        assert!(value.get("hookSpecificOutput").is_none());
    }

    /// Claude pastes exit-0 stdout. A JSON object there is a briefing the
    /// model never reads as prose.
    #[test]
    fn claude_session_start_renders_plaintext() {
        let payload = json!({
            "hook_event_name": "SessionStart",
            "session_id": "claude-1",
        });
        let rendered = render_host_stdout(HookEvent::SessionStart, &payload, &briefing_document())
            .expect("Claude sessionStart must emit a body");
        assert!(
            !rendered.trim_start().starts_with('{'),
            "Claude must stay plaintext, got {rendered}"
        );
        assert!(rendered.contains("Ask DevMap"), "{rendered}");
    }

    /// Cursor `preToolUse` blocks the tool on a schema mismatch. Emitting
    /// `additional_context` without `permission` is that mismatch.
    #[test]
    fn cursor_permission_events_render_nothing() {
        let stdout = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": "Ask DevMap before reading files.",
            }
        });
        for event_name in [
            "preToolUse",
            "beforeReadFile",
            "beforeShellExecution",
            "beforeMCPExecution",
            "beforeTabFileRead",
            "subagentStart",
        ] {
            let payload = json!({
                "cursor_version": "3.20.7",
                "hook_event_name": event_name,
            });
            assert_eq!(
                render_host_stdout(HookEvent::PreToolUse, &payload, &stdout),
                None,
                "{event_name} must fail open"
            );
        }
    }

    /// The installer wires first-nav as Cursor `postToolUse`. That event
    /// documents `additional_context` and is not a permission hook.
    #[test]
    fn cursor_post_tool_use_navigation_renders_additional_context() {
        let payload = json!({
            "cursor_version": "3.20.7",
            "hook_event_name": "postToolUse",
            "tool_name": "Read",
        });
        let stdout = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": "Ask DevMap before reading files.",
            }
        });
        let rendered = render_host_stdout(HookEvent::PreToolUse, &payload, &stdout)
            .expect("postToolUse nav must emit");
        let value: Value = serde_json::from_str(&rendered).expect("JSON");
        assert_eq!(
            value["additional_context"],
            "Ask DevMap before reading files."
        );
        assert!(value.get("permission").is_none());
    }

    /// A missing `cursor_version` must not be treated as Cursor just because
    /// the event name is camelCase.
    #[test]
    fn camel_case_event_name_alone_is_not_cursor() {
        let payload = json!({ "hook_event_name": "sessionStart" });
        let rendered = render_host_stdout(HookEvent::SessionStart, &payload, &briefing_document())
            .expect("body");
        assert!(
            !rendered.trim_start().starts_with('{'),
            "casing is not a host signal: {rendered}"
        );
    }

    /// A path-shaped session id must not escape the marker directory.
    #[test]
    fn hostile_session_ids_cannot_escape_the_marker_directory() {
        for hostile in ["../../etc/passwd", "a/b/c", "..", "."] {
            let name = session_marker_name(&json!({"session_id": hostile}));
            assert!(
                !name.contains('/'),
                "{hostile:?} produced a name with a separator: {name}"
            );
            let joined = Path::new("/base/hooks").join(&name);
            assert_eq!(
                joined.parent(),
                Some(Path::new("/base/hooks")),
                "{hostile:?} escaped its directory as {name}"
            );
        }
    }

    /// Pruning bounds the marker directory and touches nothing else.
    #[test]
    fn pruning_keeps_newest_markers_and_spares_foreign_files() {
        let root = scratch("pretool-prune");
        let dir = root.join("hooks");
        fs::create_dir_all(&dir).unwrap();

        let keeper = dir.join("hook-build.running");
        fs::write(&keeper, b"not a marker").unwrap();
        for index in 0..10 {
            fs::write(
                dir.join(format!("{MARKER_PREFIX}s{index}.{index:016x}")),
                b"",
            )
            .unwrap();
            // Distinct mtimes: the newest-first ordering is what is under test.
            thread::sleep(Duration::from_millis(6));
        }

        prune_markers(&dir, 4);
        let remaining: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let markers = remaining
            .iter()
            .filter(|name| name.starts_with(MARKER_PREFIX))
            .count();
        assert_eq!(markers, 4, "pruning kept {markers} markers, expected 4");
        assert!(
            keeper.exists(),
            "pruning removed a file that is not a marker: {remaining:?}"
        );
        // Newest-first: the survivors are the highest-numbered.
        for index in 6..10 {
            assert!(
                remaining
                    .iter()
                    .any(|name| name.starts_with(&format!("{MARKER_PREFIX}s{index}."))),
                "pruning dropped a newer marker s{index}: {remaining:?}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// An index that cannot answer must produce silence, not a directive.
    #[test]
    fn unusable_indexes_produce_no_directive() {
        let root = Path::new("/repo/Demo");
        for (label, status) in [
            (
                "not query ready",
                r#"{"query_ready":false,"node_count":10}"#,
            ),
            (
                "degraded",
                r#"{"query_ready":true,"degraded_reason":"schema","node_count":10}"#,
            ),
            ("empty", r#"{"query_ready":true,"node_count":0}"#),
            // Absent `query_ready` with a populated store. Without this case
            // the gate's default is redundant with the `nodes == 0` guard, and
            // a mutation flipping it to `unwrap_or(true)` survives — it did.
            (
                "query_ready absent on a populated store",
                r#"{"node_count":10,"edge_count":20,"generation_id":3}"#,
            ),
            ("missing fields", r#"{}"#),
            ("not json", "devmap: command not found"),
            ("empty output", ""),
        ] {
            assert!(
                compose_pre_tool_use(status, root).is_none(),
                "{label} produced a directive"
            );
        }
    }

    /// A healthy index names itself and carries the tool list.
    #[test]
    fn healthy_index_directive_names_the_tools_and_the_caveat() {
        let text = compose_pre_tool_use(healthy_status(), Path::new("/repo/DevCouncil"))
            .expect("a healthy index emits a directive");
        assert!(text.contains("DevCouncil"), "{text}");
        assert!(text.contains("11714"), "{text}");
        assert!(text.contains("devmap_search"), "{text}");
        assert!(text.contains("walk_incomplete"), "{text}");
        assert!(
            text.chars().count() <= PRE_TOOL_CONTEXT_CAP,
            "directive is {} chars, cap is {PRE_TOOL_CONTEXT_CAP}",
            text.chars().count()
        );
    }

    /// A stale index is still worth naming, but must say it is a lower bound.
    #[test]
    fn stale_index_directive_says_its_answers_are_a_lower_bound() {
        let stale = r#"{"query_ready":true,"degraded_reason":null,"node_count":5,
                        "edge_count":6,"generation_id":1,"is_fresh":false}"#;
        let text = compose_pre_tool_use(stale, Path::new("/repo/Demo"))
            .expect("a stale but queryable index still emits");
        assert!(text.contains("lower bound"), "{text}");
    }

    /// The whole point of the event: a second navigation call stays silent.
    ///
    /// Unix-only for the same reason as
    /// [`pre_tool_use_never_emits_a_permission_decision`]: the fixture execs a
    /// `#!/bin/sh` stub. `pre_tool_use_speaks_once_for_each_distinct_session`
    /// in the conformance suite covers once-per-session on every platform.
    #[cfg(unix)]
    #[test]
    fn second_navigation_call_in_a_session_is_silent() {
        let root = scratch("pretool-once");
        store_bearing(&root);
        let exe = fake_devmap(&root.join("bin"), healthy_status());
        let payload = json!({
            "session_id": "once-1",
            "cwd": root.to_string_lossy(),
            "tool_name": "Grep",
        });
        let bytes = serde_json::to_string(&payload).unwrap();

        let first = run_hook(HookEvent::PreToolUse, bytes.as_bytes(), &exe, None);
        assert!(
            first.stdout.is_some(),
            "the first navigation call must speak"
        );
        let second = run_hook(HookEvent::PreToolUse, bytes.as_bytes(), &exe, None);
        assert!(
            second.stdout.is_none(),
            "the second call repeated the directive: {:?}",
            second.stdout
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A hook whose work genuinely fails exits 1, never 2.
    ///
    /// Exit 2 means "block the agent" on Cursor and Codex, so this is the one
    /// number the failure path may never produce. The CLI cannot reach this arm
    /// — `--root` is resolved in `main` before `run_hook` is called, and with no
    /// pin `select_roots` has no failing branch — so the conformance suite
    /// exercises `main`'s handler instead and a mutation changing *this* arm to
    /// 2 survived there. Reaching it needs an in-process call with a pin.
    #[test]
    fn a_failed_hook_exits_one_never_two() {
        let missing = PathBuf::from("/nonexistent/devmap/pinned-root");
        let payload = json!({"session_id": "fail-1", "tool_name": "Read"});
        for event in [
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
        ] {
            let outcome = run_hook(
                event,
                serde_json::to_string(&payload).unwrap().as_bytes(),
                Path::new("/bin/true"),
                Some(&missing),
            );
            assert_ne!(
                outcome.exit_code,
                2,
                "{} exited 2, which blocks the agent's tool call",
                event.as_str()
            );
            assert_eq!(
                outcome.exit_code,
                1,
                "{} should report a genuine failure as exit 1",
                event.as_str()
            );
            assert!(
                outcome.stdout.is_none(),
                "{} emitted stdout on a failure",
                event.as_str()
            );
            assert!(
                outcome
                    .stderr_line
                    .as_deref()
                    .is_some_and(|line| line.contains(event.as_str())),
                "{} failure must name the event: {:?}",
                event.as_str(),
                outcome.stderr_line
            );
        }
    }

    /// A hook that cannot write its marker must still not break the tool call.
    #[test]
    fn an_unwritable_marker_directory_fails_open_and_silent() {
        let root = scratch("pretool-readonly");
        store_bearing(&root);
        let blocked = root.join("blocked");
        // A regular file where the marker directory belongs: `create_dir_all`
        // fails, which is the branch under test.
        fs::write(&blocked, b"x").unwrap();
        assert_eq!(claim_once(&blocked, "nav.x.0"), Claim::Skip);
        let _ = fs::remove_dir_all(&root);
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

    /// A `pid` that is a fifo must not hang the hook.
    ///
    /// `read_to_string` on a fifo with no writer blocks until one appears —
    /// forever, on the one code path that must always return. The staleness
    /// check runs before every detached build, so a hook that stops here stops
    /// the agent turn behind it. Bounded on its own thread so a regression
    /// fails the test instead of wedging the suite.
    #[test]
    fn a_pid_that_is_a_fifo_does_not_block_the_hook() {
        let lock = scratch("fifo-pid");
        fs::create_dir_all(&lock).expect("mkdir lock");
        let pid = lock.join("pid");
        let made = Command::new("mkfifo")
            .arg(&pid)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !made {
            eprintln!("skipped: mkfifo is unavailable");
            let _ = fs::remove_dir_all(&lock);
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let probe = lock.clone();
        std::thread::spawn(move || {
            let _ = tx.send(lock_dir_is_stale(&probe));
        });
        let verdict = rx.recv_timeout(Duration::from_secs(20));
        // The thread is left parked on the fifo if this regresses; the process
        // is a test binary that is about to exit either way.
        assert!(
            verdict.is_ok(),
            "the staleness check blocked on a fifo instead of answering"
        );
        let _ = fs::remove_file(&pid);
        let _ = fs::remove_dir_all(&lock);
    }

    /// A `pid` file far past any pid is refused rather than read.
    #[test]
    fn a_pid_file_past_its_ceiling_is_not_read() {
        let lock = scratch("huge-pid");
        fs::create_dir_all(&lock).expect("mkdir lock");
        // Well-formed at the front, so a reader that ignored the ceiling would
        // parse a live pid out of it and hold the lock.
        let mut body = std::process::id().to_string();
        body.push_str(&" ".repeat(MAX_PID_BYTES as usize * 4));
        fs::write(lock.join("pid"), body).expect("write pid");

        // Age it past the grace window: with the owner unreadable, the lock is
        // stale, which is the fail-open answer an unbounded read would miss.
        age_lock(&lock, LOCK_PID_GRACE + Duration::from_secs(5));
        assert!(
            lock_dir_is_stale(&lock),
            "an unreadable pid past the grace window is not a live owner"
        );
        let _ = fs::remove_dir_all(&lock);
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

    /// Age a lock directory so the grace window has closed.
    ///
    /// Portable: `File::set_modified` rather than shelling out to `touch`,
    /// whose relative-time flag is spelled differently on BSD and GNU.
    fn age_lock(lock: &Path, by: Duration) {
        let handle = fs::File::open(lock).expect("open lock dir");
        handle
            .set_modified(SystemTime::now() - by)
            .expect("set lock mtime");
    }

    /// A pid file is only an owner if it names a process.
    ///
    /// `kill(pid, 0)` does not mean "is this process alive" for every integer.
    /// A negative pid addresses a process *group* and `0` addresses the
    /// caller's own group, so both answer SUCCESS and the lock reads as held
    /// by a live owner forever. A truncated or half-written pid file saying
    /// `0` is the ordinary way to get there, and the repository's rebuilds
    /// then stop until LOCK_MAX_AGE expires half an hour later.
    #[test]
    fn a_nonpositive_pid_is_not_a_live_owner() {
        for text in ["0", "-1", "-12345"] {
            let base = scratch("nonpositive");
            let lock = base.join("hook-build.running");
            fs::create_dir_all(&lock).unwrap();
            fs::write(lock.join("pid"), text).unwrap();
            age_lock(&lock, LOCK_PID_GRACE + Duration::from_secs(60));

            assert!(
                lock_dir_is_stale(&lock),
                "pid {text:?} is not a process, so it cannot keep the lock held"
            );
            let _ = fs::remove_dir_all(&base);
        }
    }

    /// The grace window must still hold for a pid that cannot be read.
    ///
    /// The guard above must not turn every unreadable owner into an instant
    /// reclaim — that is the race the grace window exists to close.
    #[test]
    fn a_fresh_nonpositive_pid_is_still_inside_its_grace_window() {
        let base = scratch("nonpositive-fresh");
        let lock = base.join("hook-build.running");
        fs::create_dir_all(&lock).unwrap();
        fs::write(lock.join("pid"), "0").unwrap();

        assert!(
            !lock_dir_is_stale(&lock),
            "a lock taken moments ago must be held even when its pid is unusable"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// A regular file where the lock directory belongs must not wedge or panic.
    #[test]
    fn a_lock_path_that_is_a_regular_file_terminates() {
        let base = scratch("lockfile");
        fs::create_dir_all(&base).unwrap();
        let lock = base.join("hook-build.running");
        fs::write(&lock, b"not a directory").unwrap();

        assert!(
            !matches!(try_acquire_lock_dir(&lock), Ok(true)),
            "claimed a lock whose path is a regular file"
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
