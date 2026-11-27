//! Dev Map's own Claude Code integration, emitted and validated natively.
//!
//! Everything here is transcribed from the shipped reference documentation and
//! nothing is inferred:
//!
//! * hooks — <https://code.claude.com/docs/en/hooks>
//! * plugins — <https://code.claude.com/docs/en/plugins-reference>
//! * marketplaces — <https://code.claude.com/docs/en/plugin-marketplaces>
//! * tool names — <https://code.claude.com/docs/en/tools>
//!
//! The reason this is a validator and not just a serializer: Claude Code drops a
//! hook it cannot make sense of *quietly*. An unknown event name is "ignored at
//! runtime", a `matcher` on an event without matcher support "is silently
//! ignored", an `if` field outside a tool event means the hook "never runs", and
//! a matcher whose alternatives name nothing real simply never fires. In every
//! one of those cases a writer that only serializes reports the same success it
//! reports for a working install. So each rule below is checked at the boundary,
//! before a byte is written, and a violation is named rather than shipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// The documented contract
// ---------------------------------------------------------------------------

/// Every hook event Claude Code recognizes.
///
/// <https://code.claude.com/docs/en/hooks#hook-events>. An event outside this
/// set is reported by `claude plugin validate` as "unknown hook event" and the
/// entry is dropped at runtime, so a typo would otherwise install a hook that
/// can never fire.
pub const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "Setup",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PreToolUse",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "Notification",
    "MessageDisplay",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "Stop",
    "StopFailure",
    "TeammateIdle",
    "InstructionsLoaded",
    "ConfigChange",
    "CwdChanged",
    "DirectoryAdded",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "PreCompact",
    "PostCompact",
    "PreModelSwitch",
    "PostModelSwitch",
    "Elicitation",
    "ElicitationResult",
    "SessionEnd",
];

/// Events with no matcher support: "If you add a `matcher` field to an event
/// without matcher support, it is silently ignored."
///
/// Silently is the operative word — a matcher written here reads as a filter and
/// is not one, so the hook fires far wider than its author believes.
pub const MATCHERLESS_EVENTS: &[&str] = &[
    "CwdChanged",
    "UserPromptSubmit",
    "PostToolBatch",
    "Stop",
    "TeammateIdle",
    "TaskCreated",
    "TaskCompleted",
    "WorktreeCreate",
    "WorktreeRemove",
    "MessageDisplay",
];

/// Events whose matcher filters a tool name, and the only events on which the
/// handler-level `if` field is evaluated at all.
pub const TOOL_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
];

/// Events that can grant, deny, or alter a permission decision.
///
/// Named so Dev Map's own table can be asserted to stay off them. A code index
/// has no business inside an authorization decision, and an emitter that could
/// place one there is one edit away from widening what the tool can approve.
pub const PERMISSION_DECIDING_EVENTS: &[&str] =
    &["PreToolUse", "PermissionRequest", "PermissionDenied"];

/// `FileChanged` and `StopFailure` use a narrower exact-match character set than
/// every other event: letters, digits, `_` and `|` only. A hyphen, space, dot or
/// comma keeps the whole matcher on the regular-expression path instead.
pub const NARROW_MATCHER_EVENTS: &[&str] = &["FileChanged", "StopFailure"];

/// The five documented handler types.
pub const HANDLER_TYPES: &[&str] = &["command", "http", "mcp_tool", "prompt", "agent"];

/// Built-in tool names, from <https://code.claude.com/docs/en/tools>.
///
/// Used to flag a tool matcher that can never fire. Kept a *warning* rather than
/// an error for third-party config: a name absent here is either a typo or a
/// tool newer than this table, and the two are indistinguishable from here.
pub const BUILTIN_TOOLS: &[&str] = &[
    "Agent",
    "Artifact",
    "AskUserQuestion",
    "Bash",
    "CronCreate",
    "CronDelete",
    "CronList",
    "Edit",
    "EndConversation",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "Glob",
    "Grep",
    "ListAgents",
    "ListMcpResourcesTool",
    "LSP",
    "Monitor",
    "NotebookEdit",
    "PowerShell",
    "PushNotification",
    "Read",
    "ReadMcpResourceTool",
    "RemoteTrigger",
    "ReportFindings",
    "ScheduleWakeup",
    "SendFeedback",
    "SendMessage",
    "SendUserFile",
    "ShareOnboardingGuide",
    "Skill",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TaskUpdate",
    "TodoWrite",
    "ToolSearch",
    "WaitForMcpServers",
    "WebFetch",
    "WebSearch",
    "Workflow",
    "Write",
];

/// Events whose matcher values form a closed, documented set.
///
/// An exact-match matcher naming a value outside the set can never fire, which
/// is a dead hook reported as an installed one. Open-valued events — tool names,
/// agent types, model names, filenames, command names, MCP server names — have
/// no such set and are absent here on purpose.
fn enumerated_matcher_values(event: &str) -> Option<&'static [&'static str]> {
    Some(match event {
        "SessionStart" => &["startup", "resume", "clear", "compact", "fork"],
        "Setup" => &["init", "maintenance"],
        "SessionEnd" => &["clear", "resume", "logout", "prompt_input_exit", "other"],
        "PreCompact" | "PostCompact" => &["manual", "auto"],
        "ConfigChange" => &[
            "user_settings",
            "project_settings",
            "local_settings",
            "policy_settings",
            "skills",
        ],
        "DirectoryAdded" => &["slash_command", "register_repo_root"],
        "InstructionsLoaded" => &[
            "session_start",
            "nested_traversal",
            "path_glob_match",
            "include",
            "compact",
        ],
        "StopFailure" => &[
            "rate_limit",
            "overloaded",
            "authentication_failed",
            "oauth_org_not_allowed",
            "account_on_hold",
            "billing_error",
            "invalid_request",
            "model_not_found",
            "server_error",
            "max_output_tokens",
            "unknown",
        ],
        "Notification" => &[
            "permission_prompt",
            "idle_prompt",
            "auth_success",
            "elicitation_dialog",
            "elicitation_url_dialog",
            "elicitation_complete",
            "elicitation_response",
            "agent_needs_input",
            "agent_completed",
            "quota_auto_resume_fired",
            "quota_auto_resume_stale",
            "quota_auto_resume_disabled",
        ],
        _ => return None,
    })
}

/// The documented default `timeout`, in **seconds**, for a `command`, `http` or
/// `mcp_tool` handler on this event.
///
/// 600 everywhere, lowered to 30 on `UserPromptSubmit`, `PreModelSwitch` and
/// `PostModelSwitch`, and to 10 on `MessageDisplay`. `SessionEnd` is capped at
/// [`SESSION_END_MAX_TIMEOUT_SECS`] for Cursor/Codex compatibility.
pub fn default_timeout_secs(event: &str) -> u32 {
    match event {
        "UserPromptSubmit" | "PreModelSwitch" | "PostModelSwitch" => 30,
        "MessageDisplay" => 10,
        "SessionEnd" => SESSION_END_MAX_TIMEOUT_SECS,
        _ => 600,
    }
}

/// The only documented hard ceiling for SessionEnd across hosts: Codex caps at
/// 3 seconds, and Claude Code's plugin SessionEnd budget is tighter still. Emit
/// and accept at most 3; longer values cannot be honored on either host.
pub const SESSION_END_MAX_TIMEOUT_SECS: u32 = 3;

/// Dev Map's own ceiling, not a documented one.
///
/// The docs name defaults, not maxima, so anything above the default is only
/// unusual. A timeout longer than a day is not unusual, it is a typo — the unit
/// mistake that turns 10 seconds into 10000 and hangs a turn for nearly three
/// hours. Rejected here so that mistake cannot be written to disk.
pub const ABSURD_TIMEOUT_SECS: u32 = 86_400;

// ---------------------------------------------------------------------------
// Matchers
// ---------------------------------------------------------------------------

/// How Claude Code will read a matcher string.
///
/// <https://code.claude.com/docs/en/hooks#matcher-patterns>: `"*"`, `""` or an
/// absent field match everything; a value of only letters, digits, `_`, `-`,
/// spaces, `,` and `|` is an exact string or `|`/`,`-separated list; anything
/// else is an unanchored JavaScript regular expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatcherKind {
    /// Fires on every occurrence of the event.
    All,
    /// Exact string, or a list of them.
    Exact(Vec<String>),
    /// Unanchored JavaScript regular expression — not statically checkable here.
    Regex,
}

fn is_exact_char(c: char, narrow: bool) -> bool {
    if narrow {
        // FileChanged / StopFailure: letters, digits, `_` and `|` only.
        c.is_ascii_alphanumeric() || c == '_' || c == '|'
    } else {
        c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' ' || c == ',' || c == '|'
    }
}

/// Classify a matcher exactly as Claude Code would for this event.
pub fn classify_matcher(event: &str, matcher: &str) -> MatcherKind {
    if matcher.is_empty() || matcher == "*" {
        return MatcherKind::All;
    }
    let narrow = NARROW_MATCHER_EVENTS.contains(&event);
    if !matcher.chars().all(|c| is_exact_char(c, narrow)) {
        return MatcherKind::Regex;
    }
    // On the narrow events only `|` separates alternatives; everywhere else both
    // `|` and `,` do, with surrounding whitespace tolerated.
    let parts: Vec<String> = if narrow {
        matcher.split('|').map(str::to_string).collect()
    } else {
        matcher
            .split(['|', ','])
            .map(|p| p.trim().to_string())
            .collect()
    };
    MatcherKind::Exact(parts)
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The entry is dead, or Claude Code refuses to load it. Never written.
    Error,
    /// Suspicious, but not provably wrong from here — a tool name this table
    /// does not know, a field Claude Code ignores. Promoted by `--strict`.
    Warning,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Where in the document — `hooks.PostToolUse[0].hooks[1]`, `name`, ...
    pub location: String,
    pub message: String,
}

impl Diagnostic {
    fn error(location: impl Into<String>, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            location: location.into(),
            message: message.into(),
        }
    }
    fn warning(location: impl Into<String>, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            location: location.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {}: {}",
            self.severity.label(),
            self.location,
            self.message
        )
    }
}

/// A validation verdict, plus whether warnings were promoted to errors.
#[derive(Debug, Clone)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    pub strict: bool,
}

impl Report {
    pub fn new(diagnostics: Vec<Diagnostic>, strict: bool) -> Self {
        Report {
            diagnostics,
            strict,
        }
    }
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
    }
    /// True when nothing blocks. Under `strict`, a warning blocks too — the same
    /// rule `claude plugin validate --strict` applies.
    pub fn ok(&self) -> bool {
        if self.strict {
            self.diagnostics.is_empty()
        } else {
            self.errors().next().is_none()
        }
    }
    pub fn to_json(&self) -> Value {
        json!({
            "success": self.ok(),
            "strict": self.strict,
            "diagnostics": self.diagnostics.iter().map(|d| json!({
                "severity": d.severity.label(),
                "location": d.location,
                "message": d.message,
            })).collect::<Vec<_>>(),
        })
    }
    /// One `anyhow` error carrying every blocking diagnostic, or `Ok(())`.
    ///
    /// Every failure is reported, not just the first: an emitter that stops at
    /// the earliest problem makes the operator re-run once per mistake.
    pub fn into_result(self) -> anyhow::Result<Self> {
        if self.ok() {
            return Ok(self);
        }
        let blocking: Vec<String> = self
            .diagnostics
            .iter()
            .filter(|d| self.strict || d.severity == Severity::Error)
            .map(|d| d.to_string())
            .collect();
        anyhow::bail!(
            "{} problem(s) in the Claude Code configuration:\n  {}",
            blocking.len(),
            blocking.join("\n  ")
        )
    }
}

// ---------------------------------------------------------------------------
// Hook-block validation
// ---------------------------------------------------------------------------

/// Validation rejects bidi controls only (R2): the named subset of the shared
/// escape set, so growing the display set never narrows what is accepted.
use devmap_query::bounded::is_bidi_control as is_bidi;

fn check_timeout(event: &str, at: &str, handler: &Map<String, Value>, out: &mut Vec<Diagnostic>) {
    let Some(raw) = handler.get("timeout") else {
        return;
    };
    let Some(secs) = raw.as_f64() else {
        out.push(Diagnostic::error(
            at,
            format!("`timeout` must be a number of seconds, got {raw}"),
        ));
        return;
    };
    if !secs.is_finite() || secs <= 0.0 {
        out.push(Diagnostic::error(
            at,
            format!("`timeout` must be a positive number of seconds, got {raw}"),
        ));
        return;
    }
    if secs > f64::from(ABSURD_TIMEOUT_SECS) {
        out.push(Diagnostic::error(
            at,
            format!(
                "`timeout` is {secs} seconds. The field is in SECONDS, not milliseconds; \
                 anything over {ABSURD_TIMEOUT_SECS} outlives the session it is meant to bound"
            ),
        ));
        return;
    }
    if event == "SessionEnd" && secs > f64::from(SESSION_END_MAX_TIMEOUT_SECS) {
        out.push(Diagnostic::error(
            at,
            format!(
                "SessionEnd timeout is at most {SESSION_END_MAX_TIMEOUT_SECS} seconds \
                 (Codex max; Claude plugin budget is tighter); {secs} cannot be honored"
            ),
        ));
        return;
    }
    let default = default_timeout_secs(event);
    if secs > f64::from(default) {
        out.push(Diagnostic::warning(
            at,
            format!(
                "`timeout` of {secs}s exceeds the documented default of {default}s for {event}"
            ),
        ));
    }
    // `async: true` is documented as not enforcing `timeout` at all. Carrying one
    // anyway states a bound that does not exist.
    if handler.get("async").and_then(Value::as_bool) == Some(true) {
        out.push(Diagnostic::error(
            at,
            "`timeout` is not enforced on a hook with `async: true`; \
             remove one of the two rather than claiming a bound that never applies",
        ));
    }
}

fn check_handler(event: &str, at: &str, handler: &Value, out: &mut Vec<Diagnostic>) {
    let Some(handler) = handler.as_object() else {
        out.push(Diagnostic::error(at, "hook handler must be an object"));
        return;
    };
    match handler.get("type").and_then(Value::as_str) {
        None => out.push(Diagnostic::error(
            at,
            format!(
                "handler has no `type`; one of {} is required",
                HANDLER_TYPES.join(", ")
            ),
        )),
        Some(kind) if !HANDLER_TYPES.contains(&kind) => out.push(Diagnostic::error(
            at,
            format!(
                "{kind:?} is not a hook handler type; known types: {}",
                HANDLER_TYPES.join(", ")
            ),
        )),
        Some(kind) => {
            // Required fields per type.
            let required: &[&str] = match kind {
                "command" => &["command"],
                "http" => &["url"],
                "mcp_tool" => &["server", "tool"],
                "prompt" | "agent" => &["prompt"],
                _ => &[],
            };
            for field in required {
                if !handler.contains_key(*field) {
                    out.push(Diagnostic::error(
                        at,
                        format!("a {kind:?} handler requires `{field}`"),
                    ));
                }
            }
            if kind == "command" {
                // Cursor and Codex drop `args` and do not honour `async`. Emitting
                // either produces a bare `devmap` (exit 2 = block) on Cursor, so
                // both are refused here for every host we install into.
                if handler.contains_key("args") {
                    out.push(Diagnostic::error(
                        at,
                        "`args` is refused: Cursor and Codex run hooks as a shell-string \
                         `command` only, and Cursor drops `args` so the hook becomes bare \
                         `devmap` (exit 2 blocks the agent). Use shell-form \
                         `'<abs>' hook <event>` instead",
                    ));
                }
                if handler.contains_key("async") {
                    out.push(Diagnostic::error(
                        at,
                        "`async` is refused: Cursor and Codex do not support it; \
                         `devmap hook post-tool-use` / `session-end` detach the work themselves",
                    ));
                }
                if let Some(shell) = handler.get("shell").and_then(Value::as_str) {
                    if !matches!(shell, "bash" | "powershell") {
                        out.push(Diagnostic::error(
                            at,
                            format!("`shell` accepts \"bash\" or \"powershell\", got {shell:?}"),
                        ));
                    }
                }
                if let Some(command) = handler.get("command").and_then(Value::as_str) {
                    // A bare or relative executable is the Cursor failure mode, and
                    // a repository file named `devmap` the worse one.
                    if let Some(why) = command_word_defect(command) {
                        let trimmed = command.trim();
                        out.push(Diagnostic::error(
                            at,
                            format!(
                                "hook `command` {trimmed:?} starts with an executable that {why}; \
                                 Cursor/Codex need an absolute-path shell-form command \
                                 ('/path/to/devmap' hook <event>)"
                            ),
                        ));
                    }
                }
            }
        }
    }
    // `if` is evaluated only on tool events; elsewhere the hook never runs.
    if handler.contains_key("if") && !TOOL_EVENTS.contains(&event) {
        out.push(Diagnostic::error(
            at,
            format!(
                "`if` is only evaluated on tool events ({}); on {event} a handler with \
                 `if` set never runs",
                TOOL_EVENTS.join(", ")
            ),
        ));
    }
    // `once` is honored only in skill frontmatter.
    if handler.contains_key("once") {
        out.push(Diagnostic::warning(
            at,
            "`once` is only honored in skill frontmatter; it is ignored in settings \
             files and plugin hook configs",
        ));
    }
    check_timeout(event, at, handler, out);
}

fn check_matcher(event: &str, at: &str, matcher: &str, out: &mut Vec<Diagnostic>) {
    if matcher.chars().any(is_bidi) {
        out.push(Diagnostic::error(
            at,
            "matcher contains a bidirectional-formatting character, which renders as \
             something other than what it matches",
        ));
        return;
    }
    match classify_matcher(event, matcher) {
        MatcherKind::All => {}
        MatcherKind::Regex => {
            if NARROW_MATCHER_EVENTS.contains(&event) {
                out.push(Diagnostic::warning(
                    at,
                    format!(
                        "{matcher:?} leaves {event} on the regular-expression path — only \
                         letters, digits, `_` and `|` are exact-matched for this event"
                    ),
                ));
            }
        }
        MatcherKind::Exact(values) => {
            for value in &values {
                if value.is_empty() {
                    out.push(Diagnostic::error(
                        at,
                        format!("{matcher:?} has an empty alternative, which matches nothing"),
                    ));
                    continue;
                }
                if let Some(allowed) = enumerated_matcher_values(event) {
                    if !allowed.contains(&value.as_str()) {
                        out.push(Diagnostic::error(
                            at,
                            format!(
                                "{value:?} is not a {event} matcher value, so this entry can \
                                 never fire. Documented values: {}",
                                allowed.join(", ")
                            ),
                        ));
                    }
                } else if TOOL_EVENTS.contains(&event)
                    && !BUILTIN_TOOLS.contains(&value.as_str())
                    && !value.starts_with("mcp__")
                {
                    out.push(Diagnostic::warning(
                        at,
                        format!(
                            "{value:?} is not a built-in tool name known to this build; if it \
                             is a typo the entry can never fire"
                        ),
                    ));
                }
            }
        }
    }
}

/// Validate a `{"hooks": {...}}` document, or the bare `{...}` inside it.
pub fn validate_hooks(document: &Value) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(root) = document.as_object() else {
        out.push(Diagnostic::error("<root>", "hook config must be an object"));
        return out;
    };
    let hooks = match root.get("hooks") {
        Some(Value::Object(map)) => map,
        Some(other) => {
            out.push(Diagnostic::error(
                "hooks",
                format!("`hooks` must be an object of event name to groups, got {other}"),
            ));
            return out;
        }
        None => {
            out.push(Diagnostic::error(
                "<root>",
                "no `hooks` key; a hook config is `{\"hooks\": {\"<Event>\": [...]}}`",
            ));
            return out;
        }
    };

    for (event, groups) in hooks {
        if !HOOK_EVENTS.contains(&event.as_str()) {
            let hint = HOOK_EVENTS
                .iter()
                .find(|known| known.eq_ignore_ascii_case(event))
                .map(|known| format!(" (did you mean {known:?}?)"))
                .unwrap_or_default();
            out.push(Diagnostic::error(
                format!("hooks.{event}"),
                format!(
                    "{event:?} is not a Claude Code hook event{hint}; the entry is ignored at \
                     runtime. Known events: {}",
                    HOOK_EVENTS.join(", ")
                ),
            ));
            continue;
        }
        let Some(groups) = groups.as_array() else {
            out.push(Diagnostic::error(
                format!("hooks.{event}"),
                "each event maps to an array of matcher groups",
            ));
            continue;
        };
        // Two groups under one event with the same matcher both fire, so a
        // handler duplicated across them runs twice per event.
        let mut seen_matchers: BTreeMap<String, usize> = BTreeMap::new();
        for (i, group) in groups.iter().enumerate() {
            let at = format!("hooks.{event}[{i}]");
            let Some(group) = group.as_object() else {
                out.push(Diagnostic::error(at, "matcher group must be an object"));
                continue;
            };
            match group.get("matcher") {
                None => {
                    if let Some(first) = seen_matchers.insert("*".to_string(), i) {
                        out.push(Diagnostic::error(
                            &at,
                            format!(
                                "duplicate match-all group; [{first}] already matches every \
                                 {event}, so both groups fire on the same event"
                            ),
                        ));
                    }
                }
                Some(Value::String(matcher)) => {
                    if MATCHERLESS_EVENTS.contains(&event.as_str()) && !matcher.is_empty() {
                        out.push(Diagnostic::error(
                            &at,
                            format!(
                                "{event} has no matcher support, so {matcher:?} is silently \
                                 ignored and this group fires on every occurrence"
                            ),
                        ));
                    } else {
                        check_matcher(event, &at, matcher, &mut out);
                    }
                    let key = if matcher.is_empty() {
                        "*".to_string()
                    } else {
                        matcher.clone()
                    };
                    if let Some(first) = seen_matchers.insert(key.clone(), i) {
                        out.push(Diagnostic::error(
                            &at,
                            format!(
                                "duplicate matcher {key:?} under {event}; group [{first}] \
                                 already carries it, so every handler in both groups fires \
                                 twice per event"
                            ),
                        ));
                    }
                }
                Some(other) => out.push(Diagnostic::error(
                    &at,
                    format!("`matcher` must be a string, got {other}"),
                )),
            }
            match group.get("hooks") {
                Some(Value::Array(handlers)) if !handlers.is_empty() => {
                    for (h, handler) in handlers.iter().enumerate() {
                        check_handler(event, &format!("{at}.hooks[{h}]"), handler, &mut out);
                    }
                }
                Some(Value::Array(_)) => out.push(Diagnostic::error(
                    &at,
                    "matcher group has an empty `hooks` array, so it does nothing",
                )),
                Some(other) => out.push(Diagnostic::error(
                    &at,
                    format!("`hooks` must be an array of handlers, got {other}"),
                )),
                None => out.push(Diagnostic::error(
                    &at,
                    "matcher group has no `hooks` array of handlers",
                )),
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Plugin-manifest validation
// ---------------------------------------------------------------------------

/// Manifest keys Claude Code recognizes. Anything else loads but is reported as
/// a warning, matching `claude plugin validate`.
const MANIFEST_KEYS: &[&str] = &[
    "$schema",
    "name",
    "displayName",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "metadata",
    "skills",
    "commands",
    "agents",
    "workflows",
    "hooks",
    "mcpServers",
    "outputStyles",
    "lspServers",
    "userConfig",
    "channels",
    "dependencies",
    "defaultEnabled",
    "experimental",
];

/// Manifest keys whose values are plugin-relative paths: every string must be
/// relative and start with `./`. `skills` additionally accepts `.`.
const PATH_KEYS: &[&str] = &[
    "skills",
    "commands",
    "agents",
    "workflows",
    "outputStyles",
    "hooks",
    "mcpServers",
    "lspServers",
];

/// Marketplace names reserved for official Anthropic use. A marketplace under
/// one of these stops loading, and is reported as registered from an untrusted
/// source — a failure that looks like a broken install rather than a bad name.
const RESERVED_MARKETPLACE_NAMES: &[&str] = &[
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "claude-plugins-community",
    "claude-community",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "anthropic-agent-skills",
    "knowledge-work-plugins",
    "life-sciences",
    "claude-for-legal",
    "claude-for-financial-services",
    "financial-services-plugins",
    "first-party-plugins",
    "healthcare",
];

fn check_identifier(
    at: &str,
    kind: &str,
    name: &Value,
    out: &mut Vec<Diagnostic>,
) -> Option<String> {
    let Some(text) = name.as_str() else {
        out.push(Diagnostic::error(
            at,
            format!("`name` must be a string, got {name}"),
        ));
        return None;
    };
    if text.is_empty() {
        out.push(Diagnostic::error(at, format!("{kind} `name` is empty")));
        return None;
    }
    if text.chars().any(char::is_whitespace) {
        out.push(Diagnostic::error(
            at,
            format!("{kind} `name` {text:?} contains whitespace"),
        ));
    }
    if text.chars().any(|c| c.is_control()) {
        out.push(Diagnostic::error(
            at,
            format!("{kind} `name` contains a control character"),
        ));
    }
    if text.chars().any(is_bidi) {
        out.push(Diagnostic::error(
            at,
            format!("{kind} `name` contains a bidirectional-formatting character"),
        ));
    }
    let kebab = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !text.starts_with('-')
        && !text.ends_with('-')
        && !text.contains("--");
    if !kebab {
        out.push(Diagnostic::warning(
            at,
            format!("{kind} `name` {text:?} is not kebab-case; it is used for namespacing"),
        ));
    }
    Some(text.to_string())
}

fn check_one_path(at: &str, key: &str, text: &str, out: &mut Vec<Diagnostic>) {
    // Both "." and "./" denote the plugin root, and only `skills` accepts them.
    if key == "skills" && (text == "." || text == "./") {
        return;
    }
    if !text.starts_with("./") {
        out.push(Diagnostic::error(
            at,
            format!(
                "`{key}` path {text:?} must be relative to the plugin root and start \
                 with \"./\""
            ),
        ));
    }
}

fn check_path_field(at: &str, key: &str, value: &Value, out: &mut Vec<Diagnostic>) {
    match value {
        Value::String(text) => check_one_path(at, key, text, out),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                match item.as_str() {
                    Some(text) => check_one_path(&format!("{at}[{i}]"), key, text, out),
                    None => out.push(Diagnostic::error(
                        format!("{at}[{i}]"),
                        format!("`{key}` entries must be strings"),
                    )),
                }
            }
        }
        // `hooks`, `mcpServers` and `lspServers` also accept an inline object.
        Value::Object(inline) if matches!(key, "hooks" | "mcpServers" | "lspServers") => {
            if key == "hooks" {
                out.extend(validate_hooks(&Value::Object(inline.clone())));
            }
        }
        other => out.push(Diagnostic::error(
            at,
            format!("`{key}` must be a path string or array of them, got {other}"),
        )),
    }
}

/// Which host's `plugin.json` dialect a document is written in.
///
/// The bundle emits the same manifest shape three times, and they are not the
/// same schema. Only Cursor reads `logo`; Claude Code documents no field for
/// one and ignores it. Validating every file against the Claude key set means
/// the emitter's own Cursor manifest is reported as wrong — a warning that is
/// correct about Claude and false about the file it is aimed at, which is how
/// a warning list stops being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestDialect {
    Claude,
    Cursor,
}

impl ManifestDialect {
    /// Keys this host reads that the Claude key set does not list.
    fn extra_keys(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[],
            Self::Cursor => &["logo"],
        }
    }

    /// Named in the diagnostic, so the reader knows which host is ignoring it.
    fn host(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Cursor => "Cursor",
        }
    }

    /// The dialect a path is written in, from the directory that holds it.
    ///
    /// `.cursor-plugin/plugin.json` is Cursor's; anything else is treated as
    /// Claude's, which is the stricter of the two and so the safe default for
    /// a file whose provenance is unknown.
    pub fn of_path(path: &Path) -> Self {
        match path.parent().and_then(Path::file_name) {
            Some(dir) if dir == ".cursor-plugin" => Self::Cursor,
            _ => Self::Claude,
        }
    }
}

/// Validate a `plugin.json` document against the documented manifest schema.
pub fn validate_plugin_manifest(document: &Value) -> Vec<Diagnostic> {
    validate_plugin_manifest_in(document, ManifestDialect::Claude)
}

/// Validate a `plugin.json` written in one host's dialect.
pub fn validate_plugin_manifest_in(document: &Value, dialect: ManifestDialect) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(root) = document.as_object() else {
        out.push(Diagnostic::error("<root>", "plugin.json must be an object"));
        return out;
    };
    match root.get("name") {
        None => out.push(Diagnostic::error(
            "name",
            "`name` is the one required manifest field; without it the plugin has no \
             identity to namespace or install under",
        )),
        Some(name) => {
            check_identifier("name", "plugin", name, &mut out);
        }
    }
    for (key, value) in root {
        if !MANIFEST_KEYS.contains(&key.as_str()) && !dialect.extra_keys().contains(&key.as_str()) {
            out.push(Diagnostic::warning(
                key,
                format!(
                    "{key:?} is not a recognized manifest field; {} ignores it",
                    dialect.host()
                ),
            ));
            continue;
        }
        if PATH_KEYS.contains(&key.as_str()) {
            check_path_field(key, key, value, &mut out);
        }
        match key.as_str() {
            "keywords" | "channels" | "dependencies" => {
                if !value.is_array() {
                    out.push(Diagnostic::error(
                        key,
                        format!("`{key}` must be an array; a wrong type here fails the load"),
                    ));
                }
            }
            "author" | "userConfig" => {
                if !value.is_object() {
                    out.push(Diagnostic::error(
                        key,
                        format!("`{key}` must be an object, got {value}"),
                    ));
                }
            }
            // Documented as ignored-with-a-warning rather than a load failure.
            "metadata" | "experimental" => {
                if !value.is_object() {
                    out.push(Diagnostic::warning(
                        key,
                        format!("`{key}` must be an object to be read; a {value} is ignored"),
                    ));
                }
            }
            "defaultEnabled" => {
                if !value.is_boolean() {
                    out.push(Diagnostic::error(key, "`defaultEnabled` must be a boolean"));
                }
            }
            "version" | "description" | "displayName" | "homepage" | "repository" | "license"
            | "$schema"
                if !value.is_string() =>
            {
                out.push(Diagnostic::error(
                    key,
                    format!("`{key}` must be a string, got {value}"),
                ));
            }
            _ => {}
        }
    }
    out
}

/// Validate a `marketplace.json` document.
pub fn validate_marketplace(document: &Value) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(root) = document.as_object() else {
        out.push(Diagnostic::error(
            "<root>",
            "marketplace.json must be an object",
        ));
        return out;
    };
    match root.get("name") {
        None => out.push(Diagnostic::error("name", "`name` is required")),
        Some(name) => {
            if let Some(text) = check_identifier("name", "marketplace", name, &mut out) {
                if RESERVED_MARKETPLACE_NAMES.contains(&text.as_str()) {
                    out.push(Diagnostic::error(
                        "name",
                        format!(
                            "{text:?} is reserved for official Anthropic use; a marketplace \
                             under this name is refused as an untrusted source"
                        ),
                    ));
                }
            }
        }
    }
    match root.get("owner") {
        None => out.push(Diagnostic::error("owner", "`owner` is required")),
        Some(Value::Object(owner)) => {
            if !owner.get("name").map(Value::is_string).unwrap_or(false) {
                out.push(Diagnostic::error(
                    "owner.name",
                    "`owner.name` is required and must be a string",
                ));
            }
        }
        Some(other) => out.push(Diagnostic::error(
            "owner",
            format!("`owner` must be an object, got {other}"),
        )),
    }
    match root.get("plugins") {
        None => out.push(Diagnostic::error("plugins", "`plugins` is required")),
        Some(Value::Array(entries)) => {
            for (i, entry) in entries.iter().enumerate() {
                let at = format!("plugins[{i}]");
                let Some(entry) = entry.as_object() else {
                    out.push(Diagnostic::error(at, "each plugin entry must be an object"));
                    continue;
                };
                match entry.get("name") {
                    None => out.push(Diagnostic::error(
                        format!("{at}.name"),
                        "a plugin entry requires `name`",
                    )),
                    Some(name) => {
                        check_identifier(&format!("{at}.name"), "plugin", name, &mut out);
                    }
                }
                if !entry.contains_key("source") {
                    out.push(Diagnostic::error(
                        format!("{at}.source"),
                        "a plugin entry requires `source`",
                    ));
                }
            }
        }
        Some(other) => out.push(Diagnostic::error(
            "plugins",
            format!("`plugins` must be an array, got {other}"),
        )),
    }
    out
}

// ---------------------------------------------------------------------------
// Dev Map's own integration
// ---------------------------------------------------------------------------

pub const PLUGIN_NAME: &str = "devmap";
/// The product's name, for the one manifest field hosts render as a name.
///
/// Separate from [`PLUGIN_NAME`] on purpose: that is the install identifier
/// and must stay a kebab-case slug, so the two can never be the same string.
pub const PLUGIN_DISPLAY_NAME: &str = "Dev Map";
pub const MARKETPLACE_NAME: &str = "devmap-local";
pub const MCP_SERVER_NAME: &str = "devmap";
const PLUGIN_HOMEPAGE: &str = "https://github.com/bharathvbcr/DevCouncil";
const PLUGIN_REPOSITORY: &str = "https://github.com/bharathvbcr/DevCouncil.git";
const PLUGIN_LICENSE: &str = "MIT OR Apache-2.0";

/// Bundle-relative path of the plugin mark, and the bytes themselves.
///
/// `include_bytes!` for the same reason the skills use `include_str!`: a
/// `cargo install` binary carries no repository beside it, and a bundle whose
/// manifest names a logo that was never written is worse than one with no
/// logo — it parses, renders nothing, and no other check looks.
pub const PLUGIN_LOGO_PATH: &str = "assets/logo.png";
const PLUGIN_LOGO: &[u8] = include_bytes!("../assets/logo.png");

/// Legacy host placeholder. Emitted hooks no longer carry it — root discovery
/// happens inside `devmap hook` from stdin (`cwd`, `file_path`, `workspace_roots`,
/// apply_patch paths). Kept for documentation and older fixtures.
#[allow(dead_code)]
pub const PROJECT_DIR: &str = "${CLAUDE_PROJECT_DIR}";

/// `SessionStart` fires with one of five sources. `fork` arrived in v2.1.214 for
/// `--fork-session`; on earlier builds a forked session reports `resume`. The
/// value is only letters and `|`, so it is compared as an exact alternation list
/// — a missing entry silently never fires rather than erroring.
pub const SESSION_START_MATCHER: &str = "startup|resume|clear|compact|fork";

/// The tools that change files on disk, and so can move the index out of date.
///
/// `Bash` is here because the shell is how git operations (merge, rebase,
/// checkout, stash), in-place edits and formatters reach the tree, and none of
/// them passes through an Edit tool. A matcher cannot tell `git merge` from
/// `ls`, so the handler does: `devmap hook post-tool-use` returns at once for a
/// command that cannot change the tree and rate-limits the rest per root. Codex
/// names its shell tool `Bash` too, and this table is also its hooks.json.
/// `MultiEdit` is absent because it is not in the tool table — an exact matcher
/// naming it would be a dead alternative dressed as coverage.
pub const WRITE_TOOL_MATCHER: &str = "Edit|Write|NotebookEdit|Bash";

/// The tool whose failure can still have written: a shell command's exit code
/// says nothing about what it changed before it stopped.
pub const SHELL_TOOL_MATCHER: &str = "Bash";

/// One hook Dev Map installs, and why that event.
#[derive(Debug, Clone, Copy)]
pub struct DevmapHook {
    pub event: &'static str,
    /// `""` means every occurrence of the event.
    pub matcher: &'static str,
    /// `devmap hook <event>` name (`session-start` / `post-tool-use` / `session-end`).
    pub hook_event: &'static str,
    /// Seconds. Detached work returns within ~100 ms; this bounds the hook process.
    pub timeout_secs: Option<u32>,
    pub status_message: &'static str,
    pub purpose: &'static str,
}

/// Dev Map's hook table.
///
/// Read-only or index-only, all on events that cannot decide a permission.
/// Every other event is settled as "no handler" in [`event_coverage`], with the
/// reason, rather than left unexamined.
///
/// Handlers are shell-form only (`'<abs>' hook <event>`): Cursor drops `args`,
/// Codex ignores `async`, and exit 2 blocks the agent on both.
pub const DEVMAP_HOOKS: &[DevmapHook] = &[
    DevmapHook {
        event: "SessionStart",
        matcher: SESSION_START_MATCHER,
        hook_event: "session-start",
        timeout_secs: Some(120),
        status_message: "Dev Map index status",
        purpose: "SessionStart is one of the four events whose exit-0 stdout is added to \
                  the context Claude can see, so the agent starts the session knowing \
                  whether the index is fresh instead of trusting a stale map. \
                  `devmap hook session-start` runs status --auto-rebuild and the last \
                  session summary synchronously.",
    },
    DevmapHook {
        event: "UserPromptSubmit",
        matcher: "",
        hook_event: "user-prompt-submit",
        timeout_secs: Some(5),
        status_message: "",
        purpose: "61 of 75 recorded sessions made no DevMap query despite the SessionStart \
                  directive. `devmap hook user-prompt-submit` adds one short reminder to a \
                  turn only while the session's own record shows no query, at most three \
                  times, and goes silent at the first one. It spawns nothing and reads no \
                  log: one marker lookup per prompt.",
    },
    DevmapHook {
        event: "PostToolUse",
        matcher: WRITE_TOOL_MATCHER,
        hook_event: "post-tool-use",
        timeout_secs: Some(5),
        status_message: "",
        purpose: "An incremental rebuild after a write, including one made through the \
                  shell (git merge, sed -i, a formatter). `devmap hook post-tool-use` \
                  returns within ~100 ms and spawns a detached, coalesced build so \
                  hosts without `async` never block the agent loop; a shell command \
                  that cannot change the tree spawns nothing.",
    },
    DevmapHook {
        event: "PostToolUseFailure",
        matcher: SHELL_TOOL_MATCHER,
        hook_event: "post-tool-use",
        timeout_secs: Some(5),
        status_message: "",
        purpose: "A shell command that writes and then exits non-zero (git merge stopped \
                  on conflicts, git rebase at a conflict, git stash pop with a conflict) \
                  fires PostToolUseFailure, not PostToolUse, yet leaves a changed tree. \
                  Routed to the same `devmap hook post-tool-use`, which only spawns for a \
                  command that may change the tree. Bash only: Edit, Write and \
                  NotebookEdit that failed wrote nothing to index.",
    },
    DevmapHook {
        event: "SessionEnd",
        matcher: "",
        hook_event: "session-end",
        timeout_secs: Some(SESSION_END_MAX_TIMEOUT_SECS),
        status_message: "Dev Map session report",
        purpose: "SessionEnd is the last chance to persist what this session asked and \
                  what the index withheld. `devmap hook session-end` detaches \
                  session-report within the 3-second cross-host budget.",
    },
];

/// Every documented event, with what Dev Map does about it.
///
/// Present so "unhandled" is a decision on the record rather than an omission.
pub fn event_coverage() -> Vec<(&'static str, Option<&'static DevmapHook>, &'static str)> {
    HOOK_EVENTS
        .iter()
        .map(|event| {
            let hook = DEVMAP_HOOKS.iter().find(|h| h.event == *event);
            let reason = hook.map(|h| h.purpose).unwrap_or(match *event {
                "PreToolUse" | "PermissionRequest" | "PermissionDenied" => {
                    "Not handled: these decide an authorization outcome. A code index has \
                     nothing to contribute to one, and a handler here could only widen what \
                     Dev Map is able to approve."
                }
                "PostToolBatch" => {
                    "Not handled: PostToolUse already refreshes after a write, and a second \
                     rebuild per batch would index the same change twice."
                }
                "SessionEnd" => {
                    "Handled: session-end detaches session-report within the 3s cross-host \
                     budget. It does not build."
                }
                "Stop" | "SubagentStop" | "StopFailure" | "TeammateIdle" => {
                    "Not handled: Dev Map has no claim to verify at a stop, and blocking one \
                     on an index would be a gate it was never asked to be."
                }
                "UserPromptExpansion" | "MessageDisplay" => {
                    "Not handled: these run on the human's critical path, and index status \
                     on every prompt is noise rather than context. UserPromptSubmit is the \
                     exception, and only for a session with no recorded query."
                }
                "PreCompact" | "PostCompact" | "PreModelSwitch" | "PostModelSwitch"
                | "ConfigChange" | "Setup" | "SubagentStart" | "Notification"
                | "InstructionsLoaded" | "Elicitation" | "ElicitationResult" | "TaskCreated"
                | "TaskCompleted" => {
                    "Not handled: nothing about the code index changes at this event."
                }
                "CwdChanged" | "DirectoryAdded" | "FileChanged" | "WorktreeCreate"
                | "WorktreeRemove" => {
                    "Not handled here: these signal work outside the tool loop. `devmap serve` \
                     watches the tree directly, which covers the same changes without a hook \
                     per event."
                }
                _ => "Not handled.",
            });
            (*event, hook, reason)
        })
        .collect()
}

/// A path that must survive into JSON exactly as it is on disk.
///
/// `Path::display()` is lossy: a non-UTF-8 byte becomes U+FFFD, and the result is
/// a plausible-looking path naming a file that does not exist. A hook whose
/// `command` was mangled that way fails on every fire, into a debug log, while
/// the writer that produced it reported success.
pub(crate) fn utf8_path(label: &str, path: &Path) -> anyhow::Result<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        anyhow::anyhow!(
            "{label} is not valid UTF-8 ({}), and JSON cannot carry it without corrupting \
             the path the hook would run",
            path.display()
        )
    })
}

/// A fixture executable path that is absolute on the platform under test: a
/// path with no drive letter is not absolute on Windows, and a hook executable
/// that is not absolute is refused there, so a Unix-looking fixture would be
/// refused for the wrong reason. `unix_path` keeps its text (hostile characters
/// included); on Windows it gains a `C:` drive and backslash separators.
#[cfg(test)]
pub(crate) fn absolute_fixture(unix_path: &str) -> PathBuf {
    debug_assert!(unix_path.starts_with('/'), "{unix_path}");
    if cfg!(windows) {
        PathBuf::from(format!("C:{}", unix_path.replace('/', "\\")))
    } else {
        PathBuf::from(unix_path)
    }
}

/// [`absolute_fixture`] with forward slashes, as text: `C:/opt/x` on Windows.
/// Windows takes it as absolute, and JSON, TOML and JavaScript write it
/// unescaped, so the Unix spelling is still a substring of the written document
/// on every platform.
#[cfg(test)]
pub(crate) fn absolute_fixture_slashed(unix_path: &str) -> String {
    debug_assert!(unix_path.starts_with('/'), "{unix_path}");
    if cfg!(windows) {
        format!("C:{unix_path}")
    } else {
        unix_path.to_string()
    }
}

#[cfg(test)]
fn resolve_hook_executable(
    executable: &Path,
    path_var: Option<&std::ffi::OsStr>,
) -> anyhow::Result<PathBuf> {
    if executable.is_absolute() {
        return Ok(executable.to_path_buf());
    }
    let unresolved = || {
        anyhow::anyhow!(
            "cannot make the hook executable {} absolute; pass --binary with an absolute path",
            executable.display()
        )
    };
    if executable.components().count() != 1 {
        return executable.canonicalize().map_err(|_| unresolved());
    }
    let path_var = path_var.ok_or_else(unresolved)?;
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in executable_names(executable) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate.canonicalize().map_err(|_| unresolved());
            }
        }
    }
    Err(unresolved())
}

/// The file names one `PATH` directory is probed for, in the order a shell
/// would try them: the name as given, then (Windows only, and only for a name
/// with no extension) the name with each `PATHEXT` extension, so `devmap`
/// finds `devmap.exe`. Unix probes the name as given and nothing else.
#[cfg(test)]
fn executable_names(executable: &Path) -> Vec<std::ffi::OsString> {
    #[cfg(windows)]
    {
        executable_names_with(executable, std::env::var_os("PATHEXT").as_deref())
    }
    #[cfg(not(windows))]
    {
        vec![executable.as_os_str().to_os_string()]
    }
}

/// [`executable_names`] with `PATHEXT` passed in. An unset or empty variable
/// falls back to Windows' own default list. Extensions are used as written (the
/// Windows file system matches them case-insensitively), skipping empty entries.
#[cfg(test)]
fn executable_names_with(
    executable: &Path,
    pathext: Option<&std::ffi::OsStr>,
) -> Vec<std::ffi::OsString> {
    let mut names = vec![executable.as_os_str().to_os_string()];
    if executable.extension().is_some() {
        return names;
    }
    let pathext = pathext
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
    for ext in pathext.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let ext = ext.strip_prefix('.').unwrap_or(ext);
        let mut name = executable.as_os_str().to_os_string();
        name.push(".");
        name.push(ext);
        names.push(name);
    }
    names
}

/// The executable path a writer may put in a hook or MCP `command`: absolute,
/// or an error. A relative path is never resolved against the working
/// directory here, because that is how a file named `devmap` in a repository
/// root became the command every session ran.
pub fn absolute_executable(executable: &Path) -> anyhow::Result<PathBuf> {
    if executable.is_absolute() {
        return Ok(executable.to_path_buf());
    }
    anyhow::bail!(
        "refusing to register the relative executable {}: a host would resolve it against \
         whatever directory it runs in. Resolve it with `reachable_command` first",
        executable.display()
    )
}

/// The leading executable word of a shell-form hook command: the single- or
/// double-quoted path, else the first whitespace-delimited word.
pub(crate) fn command_executable_word(command: &str) -> &str {
    let trimmed = command.trim();
    for quote in ['\'', '"'] {
        if let Some(rest) = trimmed.strip_prefix(quote) {
            return rest.split(quote).next().unwrap_or("");
        }
    }
    trimmed.split_whitespace().next().unwrap_or("")
}

/// How one command word fares as an executable a host will run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WordVerdict {
    /// Absolute (or expanding to the user's home) and outside the repository.
    Fine,
    /// Neither absolute nor an expansion: resolved against the host's cwd or PATH.
    Relative,
    /// Led by a `$VAR` other than `$HOME`, or `~user`: the environment decides.
    Expanded,
    /// Resolves inside the repository, which can replace it.
    Inside,
}

impl WordVerdict {
    pub(crate) fn defect(self) -> Option<&'static str> {
        match self {
            Self::Fine => None,
            Self::Relative => {
                Some("is relative, so the host resolves it against whatever directory it runs in")
            }
            Self::Expanded => Some(
                "is expanded by the host from its environment, which a repository can influence",
            ),
            Self::Inside => Some("is inside the repository, which can replace it"),
        }
    }
}

/// `~`, `$HOME` or `${HOME}` alone or followed by `/…`: the rest of the word.
fn home_relative(word: &str) -> Option<&str> {
    for prefix in ["~", "${HOME}", "$HOME"] {
        if let Some(rest) = word.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with('/') {
                return Some(rest);
            }
        }
    }
    None
}

/// The one predicate for "is this command word acceptable", shared by the hook
/// validators and `doctor`: absolute and outside `repo` is fine; a leading `~`
/// or `$HOME` expanding to `home` is judged as that path; any other `$VAR`-led
/// or `~user` word, and any relative word, is a defect. An error means the word
/// could not be judged, which a caller must never read as fine.
pub(crate) fn judge_command_word(
    word: &str,
    home: Option<&Path>,
    repo: Option<&Path>,
) -> anyhow::Result<WordVerdict> {
    let resolved = if let Some(rest) = home_relative(word) {
        let home =
            home.ok_or_else(|| anyhow::anyhow!("HOME is unset, so {word:?} cannot be expanded"))?;
        home.join(rest.trim_start_matches('/'))
    } else if word.starts_with('$') || word.starts_with('~') {
        return Ok(WordVerdict::Expanded);
    } else if !Path::new(word).is_absolute() {
        return Ok(WordVerdict::Relative);
    } else {
        PathBuf::from(word)
    };
    if let Some(repo) = repo {
        if !resolved.is_dir() && crate::hook::executable_is_inside_repository(&resolved, repo)? {
            return Ok(WordVerdict::Inside);
        }
    }
    Ok(WordVerdict::Fine)
}

/// Why the leading executable word of a hook command string is not acceptable,
/// judged with `$HOME` from the environment and no repository. `None` when it is.
pub(crate) fn command_word_defect(command: &str) -> Option<String> {
    let word = command_executable_word(command);
    if word.is_empty() {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match judge_command_word(word, home.as_deref(), None) {
        Ok(verdict) => verdict.defect().map(str::to_string),
        Err(error) => Some(format!("could not be judged: {error}")),
    }
}

/// The repository the working directory is in: its git worktree top, unless
/// that is `$HOME` itself (a home directory is not the repository a hook
/// command could be planted in). `None` outside any repository.
pub fn cwd_repository() -> anyhow::Result<Option<PathBuf>> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    let home = std::env::var_os("HOME").and_then(|home| PathBuf::from(home).canonicalize().ok());
    Ok(devmap_extract::git_worktree_root(&cwd).filter(|top| Some(top) != home.as_ref()))
}

/// Subcommands this binary's parser actually registers.
///
/// The equivalent of asking the parser rather than trusting the table: a
/// renamed subcommand turns every hook that names it into a process that exits
/// non-zero on every fire, and nothing else would notice.
pub fn known_subcommands<C: clap::CommandFactory>() -> Vec<String> {
    crate::on_command_stack(|| {
        C::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect()
    })
}

/// `path` with its deepest existing ancestor canonicalized and the missing
/// tail re-appended. A tail with `..` or no file name is refused.
fn canonical_with_missing_tail(path: &Path) -> anyhow::Result<PathBuf> {
    let mut tail = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match current.canonicalize() {
            Ok(mut resolved) => {
                resolved.extend(tail.iter().rev());
                return Ok(resolved);
            }
            Err(_) => {
                let Some(name) = current.file_name().map(std::ffi::OsStr::to_os_string) else {
                    anyhow::bail!(
                        "refusing to register {}: cannot resolve where it would land",
                        path.display()
                    );
                };
                tail.push(name);
                current.pop();
            }
        }
    }
}

/// `path` without Windows' verbatim prefix when it names a drive path:
/// `\\?\C:\opt\devmap` is `C:\opt\devmap`. Canonicalizing on Windows yields the
/// verbatim form, but a host runs its command line through the ordinary Win32
/// path rules, so the entry it is given is the drive form. A verbatim UNC path
/// (`\\?\UNC\server\share`) and every non-verbatim path are returned unchanged.
fn without_verbatim_drive_prefix(path: PathBuf) -> PathBuf {
    let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) else {
        return path;
    };
    match rest.as_bytes() {
        [drive, b':', b'\\', ..] if drive.is_ascii_alphabetic() => PathBuf::from(rest),
        _ => path,
    }
}

/// What command line reaches this binary: the one owner every writer shares.
///
/// Always an absolute, canonical path that is not inside the repository at
/// `root` (nor reached through a link inside it), as
/// [`crate::hook::canonical_hook_executable`] proves for the hooks themselves.
/// A bare `devmap` is never returned, even when `devmap` on `PATH` is this
/// binary: the bare name is resolved by the *host*, against its own `PATH` and
/// working directory, and an executable `devmap` in a repository root would
/// win there. `explicit` (`--binary`) replaces the running binary and goes
/// through the same check.
pub fn reachable_command(
    executable: &Path,
    explicit: Option<&Path>,
    root: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    // A packager names the install location before anything is installed there
    // (`--binary /usr/local/bin/devmap`). Such a path cannot be canonicalized,
    // so it is judged by where it is: absolute, and not under the repository.
    if let Some(path) = explicit {
        let missing = std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        if missing && path.is_absolute() {
            // Judged where it will land: the deepest existing ancestor is
            // resolved through its links and the missing tail re-appended, so
            // `out/link/devmap` with `out/link -> repo/bin` is the repository's.
            let landing = canonical_with_missing_tail(path)?;
            let spelled = without_verbatim_drive_prefix(landing.clone());
            if let Some(root) = root.filter(|root| {
                [&landing, &spelled].into_iter().any(|candidate| {
                    crate::hook::executable_is_inside_repository(candidate, root).unwrap_or(true)
                })
            }) {
                anyhow::bail!(
                    "refusing to register the devmap command: {} lands inside the repository at {} ({})",
                    path.display(),
                    root.display(),
                    landing.display()
                );
            }
            return Ok(spelled);
        }
    }
    let chosen = explicit.unwrap_or(executable);
    match root {
        Some(root) => {
            crate::hook::canonical_hook_executable(chosen, root, "register the devmap command")
        }
        // Not in any repository: nothing can supply a file by that name, so
        // only the absolute, canonical form is required.
        None => chosen.canonicalize().map_err(|error| {
            anyhow::anyhow!(
                "cannot resolve the devmap executable {}: {error}",
                chosen.display()
            )
        }),
    }
}

/// One shell-form hook command: `'<abs>' hook <event>`.
fn hook_shell_command(exe_text: &str, event: &str) -> anyhow::Result<String> {
    let quoted = crate::hook::shell_quote_hook_executable(exe_text)?;
    Ok(format!("{quoted} hook {event}"))
}

/// Build the `{"hooks": {...}}` block Dev Map installs.
///
/// `subcommands` is the parser's own list; a missing `hook` subcommand is
/// refused rather than written.
///
/// Shell-form only: `"command": "'<abs>' hook <event>"`. The executable is
/// single-quoted so a shell does not expand `$(...)` or backticks in the
/// path. No `args`, no `async`, no `${CLAUDE_PROJECT_DIR}` — Cursor drops
/// args, Codex ignores async, and root selection happens inside the hook
/// from stdin.
pub fn hooks_block(executable: &Path, subcommands: &[String]) -> anyhow::Result<Value> {
    hooks_block_without(executable, subcommands, &[])
}

/// Events in [`DEVMAP_HOOKS`] that Codex does not document; the documented
/// list and its source page are `CODEX_DOCUMENTED_EVENTS` below. Codex has no
/// failure event; it runs `PostToolUse` for Bash after a non-zero exit too
/// (same page), so a failed shell write is still refreshed.
const CODEX_UNSUPPORTED_EVENTS: &[&str] = &["PostToolUseFailure"];

/// The hook events Codex documents (https://learn.chatgpt.com/docs/hooks, read
/// 2026-11-22). A test holds every rendered Codex `hooks.json` to this list, so
/// an event added to [`DEVMAP_HOOKS`] that Codex does not know cannot ship in
/// one unnoticed.
#[cfg(test)]
pub(crate) const CODEX_DOCUMENTED_EVENTS: &[&str] = &[
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "UserPromptSubmit",
    "SubagentStop",
    "Stop",
    "PreModelRequest",
    "PostModelRequest",
    "Interrupt",
    "SessionStart",
    "SubagentStart",
    "SessionEnd",
];

/// [`hooks_block`] for Codex's `hooks.json`: the same table minus the events
/// Codex does not document. Both Codex writers (the plugin bundle and
/// `devmap integrate`) call this, so they cannot drift.
pub fn codex_hooks_block(executable: &Path, subcommands: &[String]) -> anyhow::Result<Value> {
    hooks_block_without(executable, subcommands, CODEX_UNSUPPORTED_EVENTS)
}

fn hooks_block_without(
    executable: &Path,
    subcommands: &[String],
    omit: &[&str],
) -> anyhow::Result<Value> {
    if !subcommands.iter().any(|name| name == "hook") {
        anyhow::bail!(
            "this binary does not register `devmap hook`, so the emitted handlers \
             would fail on every fire. Known subcommands: {}",
            subcommands.join(", ")
        );
    }
    let exe = absolute_executable(executable)?;
    let exe_text = utf8_path("the devmap executable path", &exe)?;

    // Groups are keyed by (event, matcher): two handlers that fire on the same
    // event+matcher belong in one group. Two groups with the same matcher are
    // refused by Claude Code as a duplicate (both would fire, twice).
    let mut grouped: BTreeMap<(&str, &str), Vec<Value>> = BTreeMap::new();
    for spec in DEVMAP_HOOKS
        .iter()
        .filter(|spec| !omit.contains(&spec.event))
    {
        if PERMISSION_DECIDING_EVENTS.contains(&spec.event) {
            anyhow::bail!(
                "{} decides an authorization outcome. Dev Map does not install hooks on \
                 {}, because a code index has nothing to contribute to a permission \
                 decision and a handler there would widen what it can approve.",
                spec.event,
                PERMISSION_DECIDING_EVENTS.join(", ")
            );
        }
        let command = hook_shell_command(&exe_text, spec.hook_event)?;

        let mut handler = Map::new();
        handler.insert("type".into(), json!("command"));
        handler.insert("command".into(), json!(command));
        if let Some(secs) = spec.timeout_secs {
            handler.insert("timeout".into(), json!(secs));
        }
        if !spec.status_message.is_empty() {
            handler.insert("statusMessage".into(), json!(spec.status_message));
        }

        grouped
            .entry((spec.event, spec.matcher))
            .or_default()
            .push(Value::Object(handler));
    }

    let mut events: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for ((event, matcher), handlers) in grouped {
        let mut group = Map::new();
        if !matcher.is_empty() {
            group.insert("matcher".into(), json!(matcher));
        }
        group.insert("hooks".into(), Value::Array(handlers));
        events.entry(event).or_default().push(Value::Object(group));
    }

    let block = json!({
        "hooks": events
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::Array(v)))
            .collect::<Map<String, Value>>(),
    });
    // Never emit what we would refuse to accept.
    Report::new(validate_hooks(&block), true).into_result()?;
    Ok(block)
}

/// Dev Map's `plugin.json`.
///
/// `version` is omitted rather than faked when unknown: `name` is the only
/// required field, so an absent version says "unpinned" while a placeholder
/// would assert a release that does not exist.
pub fn plugin_manifest(version: Option<&str>) -> anyhow::Result<Value> {
    let mut manifest = Map::new();
    manifest.insert("name".into(), json!(PLUGIN_NAME));
    // `name` is the install identifier and must stay the kebab-case slug;
    // `displayName` is the only field Claude Code renders as a human name, so
    // without it the plugin list shows "devmap" beside products that show
    // theirs. Claude has no field for a logo at all — see
    // `cursor_plugin_manifest`, which is where the mark can actually live.
    manifest.insert("displayName".into(), json!(PLUGIN_DISPLAY_NAME));
    manifest.insert(
        "description".into(),
        json!(
            "Dev Map: symbol-level code intelligence — search, callers, blast radius, \
               dead code — over a local index."
        ),
    );
    if let Some(version) = version {
        manifest.insert("version".into(), json!(version));
    }
    manifest.insert(
        "author".into(),
        json!({"name": "DevCouncil", "url": PLUGIN_HOMEPAGE}),
    );
    manifest.insert("homepage".into(), json!(PLUGIN_HOMEPAGE));
    manifest.insert("repository".into(), json!(PLUGIN_REPOSITORY));
    manifest.insert("license".into(), json!(PLUGIN_LICENSE));
    manifest.insert(
        "keywords".into(),
        json!([
            "code-intelligence",
            "code-graph",
            "mcp",
            "devmap",
            "navigation"
        ]),
    );
    manifest.insert("skills".into(), json!("./skills"));
    let value = Value::Object(manifest);
    Report::new(validate_plugin_manifest(&value), true).into_result()?;
    Ok(value)
}

/// Codex plugin manifest written beside the Claude bundle under `.codex-plugin/`.
pub fn codex_plugin_manifest(version: Option<&str>) -> anyhow::Result<Value> {
    let mut manifest = Map::new();
    manifest.insert("name".into(), json!(PLUGIN_NAME));
    manifest.insert(
        "description".into(),
        json!(
            "Dev Map: symbol-level code intelligence for Codex — hooks refresh the \
             local index; trust via /hooks after install."
        ),
    );
    if let Some(version) = version {
        manifest.insert("version".into(), json!(version));
    }
    manifest.insert("hooks".into(), json!("./hooks/hooks.json"));
    manifest.insert("mcpServers".into(), json!("./.mcp.json"));
    manifest.insert("skills".into(), json!("./skills"));
    let value = Value::Object(manifest);
    Report::new(validate_plugin_manifest(&value), true).into_result()?;
    Ok(value)
}

/// Cursor plugin manifest, written beside the Claude bundle under
/// `.cursor-plugin/`.
///
/// Cursor is the only one of the hosts that renders a plugin logo.
/// Claude Code documents no field for one — not in `plugin.json`, not in
/// `hooks.json`, not in `.mcp.json` — and the Agent Plugins 1.0.0 schema sets
/// `additionalProperties: false`, so the same key in the portable manifest
/// makes a conformant client reject the package. The logo therefore lives
/// here and nowhere else, which is why this manifest exists at all.
pub fn cursor_plugin_manifest(version: Option<&str>) -> anyhow::Result<Value> {
    let mut manifest = Map::new();
    manifest.insert("name".into(), json!(PLUGIN_NAME));
    manifest.insert("displayName".into(), json!(PLUGIN_DISPLAY_NAME));
    manifest.insert(
        "description".into(),
        json!(
            "Dev Map: symbol-level code intelligence — search, callers, blast radius, \
               dead code — over a local index."
        ),
    );
    if let Some(version) = version {
        manifest.insert("version".into(), json!(version));
    }
    manifest.insert(
        "author".into(),
        json!({"name": "DevCouncil", "url": PLUGIN_HOMEPAGE}),
    );
    manifest.insert("homepage".into(), json!(PLUGIN_HOMEPAGE));
    manifest.insert("repository".into(), json!(PLUGIN_REPOSITORY));
    manifest.insert("license".into(), json!(PLUGIN_LICENSE));
    manifest.insert(
        "keywords".into(),
        json!([
            "code-intelligence",
            "code-graph",
            "mcp",
            "devmap",
            "navigation"
        ]),
    );
    manifest.insert("logo".into(), json!(PLUGIN_LOGO_PATH));
    manifest.insert("skills".into(), json!("./skills"));
    manifest.insert("hooks".into(), json!("./hooks/hooks.json"));
    manifest.insert("mcpServers".into(), json!("./.mcp.json"));
    let value = Value::Object(manifest);
    Report::new(
        validate_plugin_manifest_in(&value, ManifestDialect::Cursor),
        true,
    )
    .into_result()?;
    Ok(value)
}

/// Portable Agent Plugins 1.0.0 root `plugin.json` (used by Antigravity and
/// other spec-conformant clients).
///
/// Agent Plugins 1.0.0 sets `additionalProperties: false`, so host-specific
/// extensions (`displayName`, `logo`, path-override keys) stay in the host
/// manifests while `skills/` and `mcp.json` are discovered by convention.
pub fn agent_plugin_manifest(version: Option<&str>) -> anyhow::Result<Value> {
    let mut manifest = Map::new();
    manifest.insert(
        "$schema".into(),
        json!("https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"),
    );
    manifest.insert("name".into(), json!(PLUGIN_NAME));
    if let Some(version) = version {
        manifest.insert("version".into(), json!(version));
    }
    manifest.insert(
        "description".into(),
        json!(
            "Dev Map: symbol-level code intelligence — search, callers, blast radius, \
               dead code — over a local index."
        ),
    );
    manifest.insert(
        "author".into(),
        json!({"name": "DevCouncil", "url": PLUGIN_HOMEPAGE}),
    );
    manifest.insert("homepage".into(), json!(PLUGIN_HOMEPAGE));
    manifest.insert("repository".into(), json!(PLUGIN_REPOSITORY));
    manifest.insert("license".into(), json!(PLUGIN_LICENSE));
    manifest.insert(
        "keywords".into(),
        json!([
            "code-intelligence",
            "code-graph",
            "mcp",
            "devmap",
            "navigation"
        ]),
    );
    let value = Value::Object(manifest);
    Report::new(validate_plugin_manifest(&value), true).into_result()?;
    Ok(value)
}

/// Bundle files that are bytes rather than text.
///
/// Kept apart from [`render_plugin_bundle`] rather than widening its tuple:
/// every other emitted file is a document a reader can diff, and collapsing
/// the two would make `--dry-run` print a PNG. The writer emits both.
pub fn plugin_binary_assets() -> Vec<(PathBuf, &'static [u8])> {
    vec![(
        PathBuf::from(PLUGIN_NAME).join(PLUGIN_LOGO_PATH),
        PLUGIN_LOGO,
    )]
}

/// Cursor native `.cursor/hooks.json` (version 1, camelCase, flat shell commands).
///
/// Marker-owned via `generatedBy: "devmap"`. `afterFileEdit` covers Write (Tab
/// completions excluded — they are not repository edits Dev Map should index).
/// `postToolUse` with matcher `Read|Grep` is the first-nav nudge: Cursor's
/// `preToolUse` is a permission hook, and a schema mismatch there blocks the
/// tool, so the directive is injected after the first read instead. Its second
/// `postToolUse` entry, matcher `Shell` (Cursor's name for the terminal tool),
/// is the shell-write refresh; `afterFileEdit` never fires for a shell write.
pub fn cursor_hooks_document(executable: &Path) -> anyhow::Result<Value> {
    let exe = utf8_path(
        "the devmap executable path",
        &absolute_executable(executable)?,
    )?;
    let session_start = hook_shell_command(&exe, "session-start")?;
    let after_file_edit = hook_shell_command(&exe, "post-tool-use")?;
    let shell_write = after_file_edit.clone();
    let post_tool_use = hook_shell_command(&exe, "pre-tool-use")?;
    let session_end = hook_shell_command(&exe, "session-end")?;
    Ok(json!({
        "version": 1,
        "generatedBy": "devmap",
        "hooks": {
            "sessionStart": [{ "command": session_start }],
            "afterFileEdit": [{ "command": after_file_edit }],
            "postToolUse": [
                {
                    "command": post_tool_use,
                    "matcher": "Read|Grep",
                },
                {
                    "command": shell_write,
                    "matcher": "Shell",
                },
            ],
            "sessionEnd": [{
                "command": session_end,
                "timeout": SESSION_END_MAX_TIMEOUT_SECS,
            }],
        }
    }))
}

pub const CURSOR_HOOKS_MARKER: &str = "devmap";

/// Skill bodies shipped in the plugin bundle.
///
/// `include_str!` so a `cargo install` binary still emits them. Each entry is
/// `(directory name under skills/, SKILL.md contents)`.
pub const PLUGIN_SKILLS: &[(&str, &str)] = &[
    ("devmap", include_str!("../skills/devmap/SKILL.md")),
    (
        "devmap-exploring",
        include_str!("../skills/devmap-exploring/SKILL.md"),
    ),
    (
        "devmap-debugging",
        include_str!("../skills/devmap-debugging/SKILL.md"),
    ),
    (
        "devmap-impact",
        include_str!("../skills/devmap-impact/SKILL.md"),
    ),
    (
        "devmap-refactoring",
        include_str!("../skills/devmap-refactoring/SKILL.md"),
    ),
];

/// A single-repository marketplace pointing at the bundled plugin, because
/// `claude plugin install` installs from a marketplace and nothing else.
pub fn marketplace_manifest(version: Option<&str>) -> anyhow::Result<Value> {
    let mut entry = Map::new();
    entry.insert("name".into(), json!(PLUGIN_NAME));
    // The marketplace entry is what the plugin browser lists, and it carries
    // its own `displayName` rather than reading the plugin manifest's — so
    // setting one there and not here leaves the browser showing the slug.
    entry.insert("displayName".into(), json!(PLUGIN_DISPLAY_NAME));
    entry.insert("source".into(), json!(format!("./{PLUGIN_NAME}")));
    entry.insert(
        "description".into(),
        json!(
            "Dev Map's Claude Code integration: index-refresh hooks, session insights, \
               agent skills, and the code-graph MCP server."
        ),
    );
    if let Some(version) = version {
        entry.insert("version".into(), json!(version));
    }
    let value = json!({
        "name": MARKETPLACE_NAME,
        "description": "Dev Map's own marketplace: the code-intelligence index for this \
                        repository, as an installable Claude Code plugin.",
        "owner": {"name": "DevCouncil", "url": PLUGIN_HOMEPAGE},
        "plugins": [Value::Object(entry)],
    });
    Report::new(validate_marketplace(&value), true).into_result()?;
    Ok(value)
}

/// True when `path` is inside the hashed test-binary directory.
///
/// The Go harness (`backend/go_orchestrator/testsupport/testsupport.go`,
/// `testBinDir`) copies `devmap` to `target/devcouncil-testbin/devmap-<hash>/`.
/// That copy's `current_exe` is not an install: the directory goes away with
/// the worktree or with `cargo clean`, and a host that stored the path then
/// fails `posix_spawn` with ENOENT. A checkout's own `target/debug` or
/// `target/release` binary is not this directory. `mcp --print-config` still
/// names that binary, because the command is specified to be the one that ran.
pub(crate) fn path_is_hashed_test_binary(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name == "devcouncil-testbin")
    })
}

/// Why `path` is a cargo build product inside a linked worktree, if it is.
///
/// A path with a `target` component followed by a profile directory and then
/// the binary (`target/<profile>/devmap`, `target/<triple>/<profile>/devmap`) is
/// build output whatever the profile is called. Every `target` component is
/// tried, and the ancestors above it are asked whether any is a linked worktree
/// that git itself registered, through `devmap_extract::paths::main_checkout_of`:
/// that excludes submodules and any folder that merely looks like a worktree.
/// A lane's checkout is removed when it lands, and the binary goes with it, so
/// the host that stored the path fails `posix_spawn` with ENOENT. The primary
/// checkout's own `target/` is not matched: it outlives any lane. A deleted
/// worktree cannot be confirmed, so it is not named.
pub(crate) fn worktree_build_output_warning(path: &Path) -> Option<String> {
    worktree_build_output_warning_at(path).or_else(|| {
        let resolved = path.canonicalize().ok()?;
        worktree_build_output_warning_at(&resolved)
    })
}

/// `worktree_build_output_warning` for one spelling of the path.
///
/// A bare-repository common dir is not recognised as a main checkout by
/// `main_checkout_of`, so a worktree of a bare repo is not named.
fn worktree_build_output_warning_at(path: &Path) -> Option<String> {
    let components: Vec<_> = path.components().collect();
    let is_target = |c: &std::path::Component<'_>| matches!(c, std::path::Component::Normal(name) if *name == "target");
    for (index, _) in components.iter().enumerate().filter(|(_, c)| is_target(c)) {
        // `target`, a profile directory, and the file itself.
        if components.len() < index + 3 {
            continue;
        }
        let above: PathBuf = components[..index].iter().collect();
        for dir in above.ancestors() {
            if let Some(main) = devmap_extract::paths::main_checkout_of(dir) {
                return Some(format!(
                    "the MCP command {} is build output inside the linked worktree {} \
                     (main checkout {}); the entry will break when that worktree is \
                     removed. Register an installed devmap, or pass --binary.",
                    path.display(),
                    dir.display(),
                    main.display()
                ));
            }
        }
    }
    None
}

/// The `notes` line for a receipt: the warning, but only when an MCP entry
/// naming `executable` is registered after the run. Otherwise nothing will
/// break. This covers MCP entries only: the hook and plugin writers also embed
/// the binary path and are not checked (card ft-502e29ba).
pub(crate) fn mcp_receipt_note(executable: &Path, registered: bool) -> Option<String> {
    if registered {
        worktree_build_output_warning(executable)
    } else {
        None
    }
}

/// True the first time `path` is seen in this process. A poisoned lock still
/// holds a valid list, so it is recovered rather than dropping the warning.
fn first_sighting(path: &Path) -> bool {
    static SEEN: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.iter().any(|p| p == path) {
        return false;
    }
    seen.push(path.to_path_buf());
    true
}

/// Print `worktree_build_output_warning` once per path per process.
///
/// Stderr only: `mcp --print-config` keeps stdout to the JSON a host consumes.
fn warn_on_worktree_build_output(executable: &Path) {
    if let Some(message) = worktree_build_output_warning(executable) {
        if first_sighting(executable) {
            eprintln!("warning: {message}");
        }
    }
}

/// Refuse to put a hashed test binary into an MCP command; warn on a linked
/// worktree's build output.
///
/// A symlink is followed. The lexical path of a deleted worktree does not
/// exist, so a failed canonicalize is not treated as safe: the path components
/// are checked either way. A missing ordinary path stays allowed, which is
/// what the config writers' unit tests hand in. Build output in a linked
/// worktree is only warned about: `CARGO_BIN_EXE_devmap` is that path whenever
/// the integration tests run in a lane.
pub(crate) fn refuse_hashed_test_binary(executable: &Path) -> anyhow::Result<()> {
    let canonical = executable.canonicalize().ok();
    let lexical = path_is_hashed_test_binary(executable);
    let via_link = canonical.as_deref().is_some_and(path_is_hashed_test_binary);
    if !lexical && !via_link {
        warn_on_worktree_build_output(executable);
        return Ok(());
    }
    let shown = if via_link {
        canonical.unwrap_or_else(|| executable.to_path_buf())
    } else {
        executable.to_path_buf()
    };
    anyhow::bail!(
        "refusing to register an MCP command at {}: that path is a hashed test \
         binary under target/devcouncil-testbin, and it disappears when the \
         worktree is removed or cargo clean runs. Register the installed \
         devmap, not the test copy.",
        shown.display()
    )
}

/// The MCP server entry an agent host needs to reach this binary.
///
/// One owner for the formula: `devmap mcp --print-config` and the plugin bundle
/// both call this, so a host configured from one cannot reach a different server
/// than a host configured from the other.
///
/// `db` is optional. Global registration omits `--db` so the server resolves
/// the store from MCP `roots/list` then the client's cwd — that is what removes
/// the `${CLAUDE_PROJECT_DIR}` failure class. When `db` is `Some`, it is passed
/// as an override for legacy per-project configs.
pub fn mcp_entry(
    executable: &Path,
    db: Option<&Path>,
    http: Option<&str>,
) -> anyhow::Result<Value> {
    refuse_hashed_test_binary(executable)?;
    // Always the absolute path this binary validated. Emitting a bare `devmap`
    // silently hands the host whichever build wins on PATH; two installs with
    // different schemas is the skew doctor reports.
    let command = utf8_path(
        "the devmap executable path",
        &absolute_executable(executable)?,
    )?;
    Ok(match http {
        Some(address) => json!({
            "type": "http",
            "url": format!("http://{}", normalize_http_address(address)),
        }),
        None => {
            if let Some(db) = db {
                json!({
                    "type": "stdio",
                    "command": command,
                    "args": ["--db", utf8_path("the store path", db)?, "mcp"],
                })
            } else {
                json!({
                    "type": "stdio",
                    "command": command,
                    "args": ["mcp"],
                })
            }
        }
    })
}

/// Per-project belt-and-braces: `devmap --root <abs> mcp`.
///
/// Insufficient for multi-tab Cursor — that host shares one process — so callers
/// must still pass `repo_path`. `--root` only helps a single-root host whose cwd
/// is not the repository.
pub fn mcp_entry_with_root(
    executable: &Path,
    root: &Path,
    http: Option<&str>,
) -> anyhow::Result<Value> {
    refuse_hashed_test_binary(executable)?;
    if http.is_some() {
        return mcp_entry(executable, None, http);
    }
    let command = utf8_path(
        "the devmap executable path",
        &absolute_executable(executable)?,
    )?;
    Ok(json!({
        "type": "stdio",
        "command": command,
        "args": ["--root", utf8_path("the repository root", root)?, "mcp"],
    }))
}

/// A bare port means loopback. Spelling the default out is the difference
/// between serving one machine and serving a network.
pub fn normalize_http_address(address: &str) -> String {
    if address.contains(':') {
        address.to_string()
    } else {
        format!("127.0.0.1:{address}")
    }
}

/// The plugin's `.mcp.json`. Global shape: no `--db`, so the server resolves
/// from MCP roots / cwd. The `db` argument is retained so call sites do not
/// have to change shape; it is intentionally unused for the emitted entry.
pub fn plugin_mcp_config(executable: &Path, _db: &Path) -> anyhow::Result<Value> {
    Ok(json!({"mcpServers": {MCP_SERVER_NAME: mcp_entry(executable, None, None)?}}))
}

/// Agent Plugins 1.0.0 root `mcp.json` (used by Antigravity and other
/// spec-conformant clients). Carries the same global `devmap` server entry as
/// `.mcp.json` plus the spec `$schema`.
pub fn agent_plugin_mcp_config(executable: &Path, _db: &Path) -> anyhow::Result<Value> {
    Ok(json!({
        "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
        "mcpServers": {MCP_SERVER_NAME: mcp_entry(executable, None, None)?},
    }))
}

/// One file the bundle writes.
pub struct EmittedFile {
    pub path: PathBuf,
    /// False when the file already held exactly these bytes.
    pub changed: bool,
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

/// Render the whole bundle without touching the filesystem.
///
/// Split from the writing so a caller can print it, diff it, or test it without
/// a directory — and so a validation failure costs no partial write.
pub fn render_plugin_bundle(
    executable: &Path,
    db: &Path,
    version: Option<&str>,
    subcommands: &[String],
) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let plugin = PathBuf::from(PLUGIN_NAME);
    let mut files = vec![
        (
            PathBuf::from(".claude-plugin").join("marketplace.json"),
            pretty(&marketplace_manifest(version)?),
        ),
        (
            plugin.join(".claude-plugin").join("plugin.json"),
            pretty(&plugin_manifest(version)?),
        ),
        (
            plugin.join("plugin.json"),
            pretty(&agent_plugin_manifest(version)?),
        ),
        (
            plugin.join("hooks").join("hooks.json"),
            pretty(&hooks_block(executable, subcommands)?),
        ),
        (
            plugin.join(".mcp.json"),
            pretty(&plugin_mcp_config(executable, db)?),
        ),
        (
            plugin.join("mcp.json"),
            pretty(&agent_plugin_mcp_config(executable, db)?),
        ),
        (
            plugin.join(".codex-plugin").join("plugin.json"),
            pretty(&codex_plugin_manifest(version)?),
        ),
        (
            plugin.join(".cursor-plugin").join("plugin.json"),
            pretty(&cursor_plugin_manifest(version)?),
        ),
        (
            plugin
                .join(".codex-plugin")
                .join("hooks")
                .join("hooks.json"),
            pretty(&codex_hooks_block(executable, subcommands)?),
        ),
    ];
    for (name, body) in PLUGIN_SKILLS {
        let mut text = (*body).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        files.push((plugin.join("skills").join(name).join("SKILL.md"), text));
    }
    Ok(files)
}

/// Write the bundle under `out_dir`.
///
/// Every file goes through `write_atomic`: a temp file in the destination
/// directory renamed into place, with a name unique per process and per write.
/// Two `devmap claude plugin` runs against one directory therefore never leave a
/// half-written manifest for a reader to parse, and neither loses to the other's
/// rename.
pub fn write_plugin_bundle(
    out_dir: &Path,
    executable: &Path,
    db: &Path,
    version: Option<&str>,
    subcommands: &[String],
) -> anyhow::Result<Vec<EmittedFile>> {
    // Rendering first means a validation failure or a non-UTF-8 path costs no
    // partial write: nothing is created until every file is known good.
    let rendered = render_plugin_bundle(executable, db, version, subcommands)?;
    let assets = plugin_binary_assets();
    let mut written = Vec::with_capacity(rendered.len() + assets.len());
    for (relative, json) in rendered {
        let path = out_dir.join(&relative);
        let changed = devmap_query::write_atomic(&path, json.as_bytes())
            .map_err(|err| anyhow::anyhow!("could not write {}: {err}", path.display()))?;
        written.push(EmittedFile { path, changed });
    }
    // Written through the same atomic path as the documents, and reported in
    // the same list: a manifest naming a logo is only true if the logo is
    // emitted with it, so the two must not be able to ship apart.
    for (relative, bytes) in assets {
        let path = out_dir.join(&relative);
        let changed = devmap_query::write_atomic(&path, bytes)
            .map_err(|err| anyhow::anyhow!("could not write {}: {err}", path.display()))?;
        written.push(EmittedFile { path, changed });
    }
    Ok(written)
}

/// Validate a file on disk, dispatching on what it looks like.
///
/// A file that parses as none of the three shapes is an error, not a pass. A
/// validator that shrugs at input it does not recognize reports "no problems"
/// for a document it never examined.
pub fn validate_file(path: &Path, strict: bool) -> anyhow::Result<Report> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| anyhow::anyhow!("could not read {}: {err}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| anyhow::anyhow!("{} is not valid JSON: {err}", path.display()))?;
    let diagnostics = if value.get("plugins").is_some() && value.get("owner").is_some() {
        validate_marketplace(&value)
    } else if value.get("hooks").and_then(Value::as_object).is_some() {
        // Inline hooks object (settings or plugin). A string `hooks` path is a
        // plugin manifest field, not a hooks document — handled below.
        let mut out = validate_hooks(&value);
        if value.get("name").is_some() {
            out.extend(validate_plugin_manifest_in(
                &value,
                ManifestDialect::of_path(path),
            ));
        }
        out
    } else if value.get("name").is_some() {
        validate_plugin_manifest_in(&value, ManifestDialect::of_path(path))
    } else {
        vec![Diagnostic::error(
            "<root>",
            "not a recognizable Claude Code artifact: expected a hook config (`hooks`), a \
             plugin manifest (`name`), or a marketplace (`name` + `owner` + `plugins`)",
        )]
    };
    Ok(Report::new(diagnostics, strict))
}

// ---------------------------------------------------------------------------
// The copy Claude Code loads
// ---------------------------------------------------------------------------

/// Where a [`LoadedPlugin`] path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOrigin {
    /// `installPath` in `installed_plugins.json`: a copy under the cache.
    InstallRecord,
    /// A relative-path plugin of a local-directory marketplace, which Claude
    /// Code loads from the marketplace folder itself.
    InPlace,
}

/// The directory Claude Code reads its user-level settings, plugins and
/// global config from.
///
/// `CLAUDE_CONFIG_DIR` when it is set and non-empty, else `<home>/.claude`
/// (<https://code.claude.com/docs/en/settings>). This is the one owner: every
/// site that names `settings.json`, `plugins/` or the global `.claude.json`
/// goes through it, so a seat-pinned profile is read as the profile a session
/// actually loads. A relative value is made absolute against the current
/// directory, the way a process that inherited it resolves it; symlinks are
/// left alone.
pub fn claude_config_dir(home: &Path) -> PathBuf {
    config_dir_from(home, std::env::var_os("CLAUDE_CONFIG_DIR"))
}

fn config_dir_from(home: &Path, env: Option<std::ffi::OsString>) -> PathBuf {
    match env.filter(|value| !value.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            std::path::absolute(&dir).unwrap_or(dir)
        }
        None => home.join(".claude"),
    }
}

/// Claude Code's global config document (`mcpServers`, per-project state).
///
/// Claude Code keeps it at `$CLAUDE_CONFIG_DIR/.claude.json` when the variable
/// is set and at `~/.claude.json` otherwise. The first is confirmed by a seat
/// profile on disk (`~/.claude-chandra/.claude.json` registering the MCP
/// servers that profile loads); the second is the unset default.
pub fn claude_global_config(home: &Path) -> PathBuf {
    global_config_from(home, std::env::var_os("CLAUDE_CONFIG_DIR"))
}

fn global_config_from(home: &Path, env: Option<std::ffi::OsString>) -> PathBuf {
    match env.as_ref().filter(|value| !value.is_empty()) {
        Some(_) => config_dir_from(home, env).join(".claude.json"),
        None => home.join(".claude.json"),
    }
}

/// One copy of the Dev Map plugin that Claude Code sessions load.
pub struct LoadedPlugin {
    pub dir: PathBuf,
    /// The version this copy declares: the version directory's name for a
    /// cached install, `.claude-plugin/plugin.json`'s `version` for an
    /// in-place one. `None` when an in-place copy declares none.
    pub version: Option<String>,
    pub origin: LoadOrigin,
}

/// What Claude Code records about the Dev Map plugin under one home.
pub struct ClaudePluginInstall {
    /// `~/.claude/plugins/cache/<marketplace>/<plugin>`.
    pub cache: PathBuf,
    /// The copies sessions load. See [`claude_plugin_install`] for how the
    /// in-place copy of a local marketplace takes precedence over the record.
    pub loaded: Vec<LoadedPlugin>,
    /// Version directories `installed_plugins.json` names under
    /// [`Self::cache`]. The loaded copies unless the plugin loads in place.
    pub recorded: Vec<PathBuf>,
    /// `enabledPlugins` in `~/.claude/settings.json` for the plugin's key.
    /// `None` when no setting names it.
    pub enabled: Option<bool>,
    /// Why the install record could not be read, when it exists but is not
    /// usable. A record nobody could read is not a record naming nothing.
    pub record_error: Option<String>,
    /// Why the marketplace could not be resolved to the copy it loads. Set,
    /// [`Self::loaded`] falls back to the install record, which may not be
    /// what a session runs.
    pub load_error: Option<String>,
    /// Version directories under the cache that the record does not name.
    pub leftovers: Vec<PathBuf>,
}

/// One Claude Code profile directory and its paired global config document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeProfile {
    /// Host bucket reported by `devmap doctor`: `"claude-code"` for the active
    /// profile (`CLAUDE_CONFIG_DIR` or `~/.claude`), and
    /// `"claude-code (<dir>)"` for sibling profiles such as `~/.claude-chandra`.
    pub host_label: String,
    pub config_dir: PathBuf,
    pub global_config: PathBuf,
    pub is_active: bool,
}

/// Enumerate every Claude Code profile under `home`, starting with the active
/// profile (`CLAUDE_CONFIG_DIR` or `~/.claude`), followed by `~/.claude` (when
/// `CLAUDE_CONFIG_DIR` points elsewhere and default state exists) and every
/// sibling `~/.claude-*` directory holding `.claude.json`, `settings.json`, or
/// `plugins/`.
pub fn claude_profiles(home: &Path) -> Vec<ClaudeProfile> {
    let canonical_key = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let active_dir = claude_config_dir(home);
    let active_global = claude_global_config(home);
    let mut seen = vec![canonical_key(&active_dir)];
    let mut profiles = vec![ClaudeProfile {
        host_label: "claude-code".to_string(),
        config_dir: active_dir,
        global_config: active_global,
        is_active: true,
    }];

    let default_dir = home.join(".claude");
    let default_global = home.join(".claude.json");
    let default_key = canonical_key(&default_dir);
    if !seen.contains(&default_key) && (default_dir.is_dir() || default_global.is_file()) {
        seen.push(default_key);
        profiles.push(ClaudeProfile {
            host_label: "claude-code (~/.claude)".to_string(),
            config_dir: default_dir,
            global_config: default_global,
            is_active: false,
        });
    }

    let mut siblings: Vec<(String, PathBuf)> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            if !name.starts_with(".claude-") {
                return None;
            }
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let has_state = path.join(".claude.json").is_file()
                || path.join("settings.json").is_file()
                || path.join("plugins").is_dir();
            has_state.then_some((name, path))
        })
        .collect();
    siblings.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, dir) in siblings {
        let key = canonical_key(&dir);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let global_config = dir.join(".claude.json");
        profiles.push(ClaudeProfile {
            host_label: format!("claude-code ({name})"),
            config_dir: dir,
            global_config,
            is_active: false,
        });
    }
    profiles
}

/// Whether a settings file Claude Code reads names the project `.mcp.json`
/// server `name` in `disabledMcpjsonServers`.
///
/// "A rejection in any settings file applies" and "blocks it in every
/// permission mode" (<https://code.claude.com/docs/en/settings-reference#disabledmcpjsonservers>),
/// so a rejected server is never loaded. Reads user `settings.json` and the
/// project's `settings.json` / `settings.local.json`; managed settings are not
/// read, so a server rejected only there is still counted — an over-report,
/// never a missed duplicate. An unreadable file rejects nothing.
pub fn claude_project_server_rejected(home: Option<&Path>, project: &Path, name: &str) -> bool {
    let config_dir = home.map(claude_config_dir);
    claude_project_server_rejected_in(config_dir.as_deref(), project, name)
}

/// [`claude_project_server_rejected`] scoped to an explicit Claude profile dir.
pub fn claude_project_server_rejected_in(
    config_dir: Option<&Path>,
    project: &Path,
    name: &str,
) -> bool {
    let project_settings = project.join(".claude");
    config_dir
        .map(|dir| dir.join("settings.json"))
        .into_iter()
        .chain([
            project_settings.join("settings.json"),
            project_settings.join("settings.local.json"),
        ])
        .any(|path| {
            crate::integrate::read_host_config(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|settings| {
                    settings
                        .get("disabledMcpjsonServers")?
                        .as_array()
                        .map(|names| names.iter().any(|n| n.as_str() == Some(name)))
                })
                .unwrap_or(false)
        })
}

/// Which copy of the Dev Map plugin Claude Code loads, and what it records.
///
/// A plugin whose marketplace was added from a local directory, and whose
/// marketplace entry names a relative source, "loads in place from its path
/// inside the marketplace folder" and "needs no install record because it
/// loads from the marketplace itself"
/// (<https://code.claude.com/docs/en/plugins/loading>). `installed_plugins.json`
/// may still name a cache copy for it — Claude Code 2.1.292 reports that
/// `installPath` beside a different `readFromFolder` — and a check that read
/// the cache copy inspected files no session loads. Every other marketplace
/// copies the plugin into the cache and loads that copy, so for those the
/// install record is the answer.
pub fn claude_plugin_install(home: &Path) -> ClaudePluginInstall {
    claude_plugin_install_in(&claude_config_dir(home))
}

/// [`claude_plugin_install`] for one explicit Claude Code profile directory.
pub fn claude_plugin_install_in(config_dir: &Path) -> ClaudePluginInstall {
    let plugins = config_dir.join("plugins");
    let cache = plugins
        .join("cache")
        .join(MARKETPLACE_NAME)
        .join(PLUGIN_NAME);
    let record = plugins.join("installed_plugins.json");
    let mut recorded: Vec<PathBuf> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut record_error = None;
    match std::fs::read_to_string(&record) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => match value.get("plugins").and_then(Value::as_object) {
                // Matched by where the install lives, not by spelling the key:
                // the key is `<plugin>@<marketplace>`, and the cache path is the
                // part this binary owns.
                Some(map) => {
                    for (key, installs) in map {
                        let paths = installs
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|install| install.get("installPath")?.as_str())
                            .map(PathBuf::from);
                        for path in paths {
                            if path.parent() == Some(cache.as_path()) && !recorded.contains(&path) {
                                recorded.push(path);
                                if !keys.contains(key) {
                                    keys.push(key.clone());
                                }
                            }
                        }
                    }
                }
                None => {
                    record_error = Some(format!("{} has no `plugins` object", record.display()))
                }
            },
            Err(error) => record_error = Some(format!("{} is not JSON: {error}", record.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => record_error = Some(format!("{}: {error}", record.display())),
    }
    let (in_place, load_error) = match in_place_plugin_dir(&plugins) {
        Ok(dir) => (dir, None),
        Err(error) => (None, Some(error)),
    };
    let loaded = match in_place {
        Some(dir) => {
            // An in-place plugin may have no record at all; its enabled flag
            // is still keyed `<plugin>@<marketplace>`.
            let key = format!("{PLUGIN_NAME}@{MARKETPLACE_NAME}");
            if !keys.contains(&key) {
                keys.push(key);
            }
            vec![LoadedPlugin {
                version: manifest_version(&dir),
                dir,
                origin: LoadOrigin::InPlace,
            }]
        }
        None => recorded
            .iter()
            .map(|dir| LoadedPlugin {
                dir: dir.clone(),
                version: dir.file_name().and_then(|n| n.to_str()).map(str::to_string),
                origin: LoadOrigin::InstallRecord,
            })
            .collect(),
    };
    let enabled = std::fs::read_to_string(config_dir.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| {
            let map = settings.get("enabledPlugins")?.as_object()?;
            let flags: Vec<bool> = keys
                .iter()
                .filter_map(|key| map.get(key)?.as_bool())
                .collect();
            (!flags.is_empty()).then(|| flags.contains(&true))
        });
    let mut leftovers: Vec<PathBuf> = std::fs::read_dir(&cache)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|dir| dir.is_dir() && !recorded.contains(dir))
                .collect()
        })
        .unwrap_or_default();
    leftovers.sort();
    ClaudePluginInstall {
        cache,
        loaded,
        recorded,
        enabled,
        record_error,
        load_error,
        leftovers,
    }
}

/// Enabled non-`devmap` plugins in `config_dir` that ship a `.mcp.json` or
/// `mcp.json`, returned as `(plugin_key, mcp_config_path)`.
///
/// Used by `devmap doctor` to detect embedded `devmap_*` providers shipped
/// inside plugins such as `waylode@*` or `gitpulse@*`.
pub fn claude_enabled_other_plugin_mcps_in(config_dir: &Path) -> Vec<(String, PathBuf)> {
    let settings_text = match std::fs::read_to_string(config_dir.join("settings.json")) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    let Ok(settings) = serde_json::from_str::<Value>(&settings_text) else {
        return Vec::new();
    };
    let Some(enabled_map) = settings.get("enabledPlugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    let plugins = config_dir.join("plugins");
    let installed_map: Option<Map<String, Value>> =
        std::fs::read_to_string(plugins.join("installed_plugins.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| v.get("plugins")?.as_object().cloned());

    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut keys: Vec<(&String, bool)> = enabled_map
        .iter()
        .filter_map(|(k, v)| Some((k, v.as_bool()?)))
        .filter(|(_, on)| *on)
        .collect();
    keys.sort_by(|a, b| a.0.cmp(b.0));

    for (key, _) in keys {
        let Some((plugin_name, marketplace)) = key.split_once('@') else {
            continue;
        };
        if plugin_name == PLUGIN_NAME {
            continue;
        }
        let mut dirs = Vec::new();
        if let Ok(Some(dir)) = in_place_named_plugin_dir(&plugins, marketplace, plugin_name) {
            dirs.push(dir);
        } else if let Some(map) = &installed_map {
            if let Some(installs) = map.get(key).and_then(Value::as_array) {
                for install in installs {
                    if let Some(path) = install.get("installPath").and_then(Value::as_str) {
                        dirs.push(PathBuf::from(path));
                    }
                }
            }
        }
        for dir in dirs {
            let mcp = [".mcp.json", "mcp.json"]
                .into_iter()
                .map(|name| dir.join(name))
                .find(|p| p.is_file());
            if let Some(mcp) = mcp {
                let canon = mcp.canonicalize().unwrap_or_else(|_| mcp.clone());
                if !seen.contains(&canon) {
                    seen.push(canon);
                    out.push((key.clone(), mcp));
                }
            }
        }
    }
    out
}

/// The in-place plugin directory, when the Dev Map marketplace is a local
/// directory whose entry for the plugin is a relative path.
///
/// `Ok(None)` is a positive answer: no such marketplace is recorded, it is a
/// remote source whose plugins are copied into the cache, or its entry is not
/// a relative path. `Err` means the question could not be answered.
fn in_place_plugin_dir(plugins: &Path) -> Result<Option<PathBuf>, String> {
    in_place_named_plugin_dir(plugins, MARKETPLACE_NAME, PLUGIN_NAME)
}

fn in_place_named_plugin_dir(
    plugins: &Path,
    marketplace_name: &str,
    plugin_name: &str,
) -> Result<Option<PathBuf>, String> {
    let known = plugins.join("known_marketplaces.json");
    let text = match std::fs::read_to_string(&known) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", known.display())),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not JSON: {error}", known.display()))?;
    let Some(entry) = value.get(marketplace_name) else {
        return Ok(None);
    };
    let root = match entry.pointer("/source/source").and_then(Value::as_str) {
        // `installLocation` is "the path you gave" for a local marketplace,
        // and the directory source's `path` is the marketplace root.
        Some("directory") => entry
            .get("installLocation")
            .or_else(|| entry.pointer("/source/path"))
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| {
                format!(
                    "{}: the `{marketplace_name}` directory marketplace names no path",
                    known.display()
                )
            })?,
        // Read in place too, but the documentation states the in-place rule
        // for directories, not for a marketplace file; resolving it would be
        // a guess, and a guess here decides which copy every check reads.
        Some("file") => {
            return Err(format!(
                "{}: the `{marketplace_name}` marketplace is a `file` source, whose plugin \
                 location is not resolved here",
                known.display()
            ))
        }
        _ => return Ok(None),
    };
    let manifest = root.join(".claude-plugin").join("marketplace.json");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|error| format!("{}: {error}", manifest.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not JSON: {error}", manifest.display()))?;
    let source = value
        .get("plugins")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|plugin| plugin.get("name").and_then(Value::as_str) == Some(plugin_name))
        .and_then(|plugin| plugin.get("source")?.as_str());
    let Some(source) = source.filter(|s| s.starts_with("./") || s.starts_with("../")) else {
        return Ok(None);
    };
    // Relative sources resolve from the marketplace root. `./` segments are
    // dropped so the path reads the way Claude Code reports it.
    let relative: PathBuf = Path::new(source)
        .components()
        .filter(|part| !matches!(part, std::path::Component::CurDir))
        .collect();
    Ok(Some(root.join(relative)))
}

/// `version` from a plugin directory's `.claude-plugin/plugin.json`.
fn manifest_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(".claude-plugin").join("plugin.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get("version")?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `plugin_command` answers the bare name when `devmap` on `PATH` is this
    /// binary (CI puts `target/debug` on `PATH`); a hook must still be absolute.
    #[test]
    fn a_bare_hook_executable_is_resolved_on_path_and_never_left_bare() {
        let dir = std::env::temp_dir().join(format!("devmap-hook-exe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("devmap"), b"x").unwrap();
        let path_var = std::env::join_paths([Path::new(""), dir.as_path()]).unwrap();

        let resolved =
            resolve_hook_executable(Path::new("devmap"), Some(path_var.as_os_str())).unwrap();
        assert!(resolved.is_absolute(), "{resolved:?}");
        assert!(resolved.ends_with("devmap"), "{resolved:?}");

        let missing =
            resolve_hook_executable(Path::new("no-such-devmap"), Some(path_var.as_os_str()));
        assert!(missing.is_err(), "an unresolvable name must be refused");
        assert!(resolve_hook_executable(Path::new("devmap"), None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows: `--binary devmap` with only `devmap.exe` on `PATH` resolves,
    /// trying `PATHEXT` in order (default list when unset), and a name that
    /// already has an extension is never expanded. The name-building runs on
    /// every platform; the on-disk resolution is Windows-only.
    #[test]
    fn pathext_expands_a_bare_name_in_order_and_leaves_an_extension_alone() {
        let names = |name: &str, pathext: Option<&str>| -> Vec<String> {
            executable_names_with(Path::new(name), pathext.map(std::ffi::OsStr::new))
                .into_iter()
                .map(|n| n.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            names("devmap", Some(".EXE;.cmd; ;.Bat")),
            ["devmap", "devmap.EXE", "devmap.cmd", "devmap.Bat"]
        );
        assert_eq!(
            names("devmap", None),
            [
                "devmap",
                "devmap.COM",
                "devmap.EXE",
                "devmap.BAT",
                "devmap.CMD"
            ]
        );
        assert_eq!(names("devmap", Some("")), names("devmap", None));
        assert_eq!(names("devmap.exe", Some(".EXE")), ["devmap.exe"]);
    }

    /// A canonical Windows path is verbatim (`\\?\C:\...`); the host entry is
    /// the drive form. A verbatim UNC path and an ordinary path are unchanged.
    #[test]
    fn a_verbatim_drive_path_is_written_in_its_drive_form() {
        let strip = |text: &str| without_verbatim_drive_prefix(PathBuf::from(text));
        assert_eq!(
            strip(r"\\?\C:\opt\devcouncil-test\bin\devmap"),
            PathBuf::from(r"C:\opt\devcouncil-test\bin\devmap")
        );
        assert_eq!(
            strip(r"\\?\UNC\server\share\devmap"),
            PathBuf::from(r"\\?\UNC\server\share\devmap")
        );
        assert_eq!(strip(r"\\?\C:"), PathBuf::from(r"\\?\C:"));
        assert_eq!(
            strip("/usr/local/bin/devmap"),
            PathBuf::from("/usr/local/bin/devmap")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_bare_hook_executable_finds_its_exe_through_pathext_on_windows() {
        let dir = std::env::temp_dir().join(format!("devmap-hook-exe-win-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("devmap.exe"), b"x").unwrap();
        let path_var = std::env::join_paths([dir.as_path()]).unwrap();
        let resolved =
            resolve_hook_executable(Path::new("devmap"), Some(path_var.as_os_str())).unwrap();
        assert!(resolved.is_absolute(), "{resolved:?}");
        assert!(
            resolved
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("devmap.exe")),
            "{resolved:?}"
        );
        assert!(
            resolve_hook_executable(Path::new("no-such-devmap"), Some(path_var.as_os_str()))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_config_dir_is_the_variable_when_set_and_non_empty() {
        let home = Path::new("/home/u");
        let os = |value: &str| Some(std::ffi::OsString::from(value));
        assert_eq!(config_dir_from(home, None), home.join(".claude"));
        assert_eq!(config_dir_from(home, os("")), home.join(".claude"));
        // A drive-less `/seat/one` is not absolute on Windows and gains the
        // current drive, so the fixture is absolute on every platform.
        let seat = absolute_fixture("/seat/one");
        assert_eq!(config_dir_from(home, os(seat.to_str().unwrap())), seat);
        let relative = config_dir_from(home, os("seat"));
        assert!(relative.is_absolute(), "{relative:?}");
        assert!(relative.ends_with("seat"), "{relative:?}");
    }

    #[test]
    fn the_global_config_follows_the_profile() {
        let home = Path::new("/home/u");
        let os = |value: &str| Some(std::ffi::OsString::from(value));
        assert_eq!(global_config_from(home, None), home.join(".claude.json"));
        assert_eq!(global_config_from(home, os("")), home.join(".claude.json"));
        let seat = absolute_fixture("/seat/one");
        assert_eq!(
            global_config_from(home, os(seat.to_str().unwrap())),
            seat.join(".claude.json")
        );
    }

    #[test]
    fn matcher_classification_follows_the_documented_character_rules() {
        assert_eq!(classify_matcher("PreToolUse", ""), MatcherKind::All);
        assert_eq!(classify_matcher("PreToolUse", "*"), MatcherKind::All);
        assert_eq!(
            classify_matcher("PreToolUse", "Edit|Write"),
            MatcherKind::Exact(vec!["Edit".into(), "Write".into()])
        );
        // Comma plus surrounding whitespace is the exact-list path too.
        assert_eq!(
            classify_matcher("PreToolUse", "Edit, Write"),
            MatcherKind::Exact(vec!["Edit".into(), "Write".into()])
        );
        // A dot is not in the exact set, so the whole value is a regex.
        assert_eq!(
            classify_matcher("PreToolUse", "mcp__memory__.*"),
            MatcherKind::Regex
        );
        // Narrow events exclude the hyphen the others allow.
        assert_eq!(
            classify_matcher("PostToolUse", "code-reviewer"),
            MatcherKind::Exact(vec!["code-reviewer".into()])
        );
        assert_eq!(
            classify_matcher("FileChanged", "code-reviewer"),
            MatcherKind::Regex
        );
    }

    #[test]
    fn every_documented_event_is_classified_exactly_once() {
        // The tables are read by index into HOOK_EVENTS; a name in one and not
        // the other is a rule that quietly applies to nothing.
        for event in MATCHERLESS_EVENTS
            .iter()
            .chain(TOOL_EVENTS)
            .chain(NARROW_MATCHER_EVENTS)
            .chain(PERMISSION_DECIDING_EVENTS)
        {
            assert!(
                HOOK_EVENTS.contains(event),
                "{event} is classified but is not a known event"
            );
        }
        for event in MATCHERLESS_EVENTS {
            assert!(
                enumerated_matcher_values(event).is_none(),
                "{event} has no matcher support, so it cannot have matcher values"
            );
        }
        assert_eq!(
            HOOK_EVENTS.len(),
            HOOK_EVENTS
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            "HOOK_EVENTS has a duplicate"
        );
    }

    /// Every diagnostic the validator can raise, and the message it carries.
    fn diagnose(document: Value) -> Vec<String> {
        validate_hooks(&document)
            .into_iter()
            .map(|d| d.to_string())
            .collect()
    }

    fn one_handler(event: &str, group: Value) -> Value {
        json!({"hooks": {event: [group]}})
    }

    fn command_group(matcher: Option<&str>, extra: Value) -> Value {
        let mut handler =
            json!({"type": "command", "command": absolute_fixture_slashed("/bin/true")});
        if let Some(map) = extra.as_object() {
            for (k, v) in map {
                handler[k] = v.clone();
            }
        }
        match matcher {
            Some(m) => json!({"matcher": m, "hooks": [handler]}),
            None => json!({"hooks": [handler]}),
        }
    }

    #[test]
    fn an_unknown_event_is_refused_and_the_near_miss_is_named() {
        let found = diagnose(one_handler("PostToolUsage", command_group(None, json!({}))));
        assert!(
            found
                .iter()
                .any(|d| d.contains("not a Claude Code hook event")),
            "{found:?}"
        );
        // Case-only typos are the ones a reader's eye slides over.
        let cased = diagnose(one_handler("posttooluse", command_group(None, json!({}))));
        assert!(
            cased
                .iter()
                .any(|d| d.contains("did you mean \"PostToolUse\"")),
            "a case-only typo must name the event it nearly is: {cased:?}"
        );
    }

    #[test]
    fn a_matcher_on_an_event_without_matcher_support_is_refused() {
        // Claude Code ignores it silently, so the group fires on every Stop
        // while its author reads the file as filtered.
        let found = diagnose(one_handler("Stop", command_group(Some("Bash"), json!({}))));
        assert!(
            found
                .iter()
                .any(|d| d.contains("no matcher support") && d.contains("silently ignored")),
            "{found:?}"
        );
        // An empty matcher on the same event is honest and must pass.
        assert!(
            validate_hooks(&one_handler("Stop", command_group(Some(""), json!({})))).is_empty(),
            "an empty matcher states no filter and is not a mistake"
        );
    }

    #[test]
    fn duplicate_matchers_under_one_event_are_refused() {
        let doc = json!({"hooks": {"PostToolUse": [
            command_group(Some("Edit|Write"), json!({})),
            command_group(Some("Edit|Write"), json!({})),
        ]}});
        let found = diagnose(doc);
        assert!(
            found
                .iter()
                .any(|d| d.contains("duplicate matcher") && d.contains("twice")),
            "two groups with one matcher both fire: {found:?}"
        );
        // Two match-all groups are the same defect wearing a different shape.
        let all = diagnose(json!({"hooks": {"Stop": [
            command_group(None, json!({})),
            command_group(None, json!({})),
        ]}}));
        assert!(
            all.iter().any(|d| d.contains("duplicate match-all")),
            "{all:?}"
        );
    }

    #[test]
    fn absurd_and_impossible_timeouts_are_refused() {
        let cases: [(&str, Value, &str); 6] = [
            ("PostToolUse", json!(0), "positive"),
            ("PostToolUse", json!(-5), "positive"),
            ("PostToolUse", json!("10"), "must be a number"),
            // The millisecond mistake: 150000 reads as 41.7 hours.
            ("PostToolUse", json!(150_000), "SECONDS"),
            // Documented ceiling, not a guess.
            ("SessionEnd", json!(300), "at most 3"),
            ("PostToolUse", json!(f64::INFINITY), "must be a number"),
        ];
        for (event, timeout, needle) in cases {
            let found = diagnose(one_handler(
                event,
                command_group(Some(""), json!({"timeout": timeout})),
            ));
            assert!(
                found
                    .iter()
                    .any(|d| d.contains(needle) && d.starts_with("error")),
                "timeout {timeout} on {event} must be an error mentioning {needle:?}: {found:?}"
            );
        }
        // Above the documented default but not absurd: a warning, not a refusal.
        // The docs name defaults, not maxima, and claiming otherwise would be
        // inventing a rule.
        let warned = diagnose(one_handler(
            "MessageDisplay",
            command_group(None, json!({"timeout": 60})),
        ));
        assert!(
            warned
                .iter()
                .any(|d| d.starts_with("warning") && d.contains("documented default")),
            "{warned:?}"
        );
        assert!(
            !warned.iter().any(|d| d.starts_with("error")),
            "an over-default timeout is unusual, not invalid: {warned:?}"
        );
    }

    #[test]
    fn args_and_async_are_refused_for_cursor_codex_compatibility() {
        let with_args = diagnose(one_handler(
            "PostToolUse",
            command_group(Some("Edit"), json!({"args": ["build"]})),
        ));
        assert!(
            with_args
                .iter()
                .any(|d| d.contains("`args` is refused") && d.starts_with("error")),
            "{with_args:?}"
        );
        let with_async = diagnose(one_handler(
            "PostToolUse",
            command_group(Some("Edit"), json!({"async": true})),
        ));
        assert!(
            with_async
                .iter()
                .any(|d| d.contains("`async` is refused") && d.starts_with("error")),
            "{with_async:?}"
        );
    }

    #[test]
    fn a_timeout_on_an_async_hook_is_refused_because_it_is_never_enforced() {
        let found = diagnose(one_handler(
            "PostToolUse",
            command_group(Some("Edit"), json!({"async": true, "timeout": 30})),
        ));
        assert!(
            found
                .iter()
                .any(|d| d.contains("`async` is refused") || d.contains("not enforced")),
            "async must be refused for cross-host compatibility: {found:?}"
        );
    }

    #[test]
    fn an_if_field_outside_a_tool_event_is_refused() {
        let found = diagnose(one_handler(
            "SessionStart",
            command_group(Some("startup"), json!({"if": "Bash(git *)"})),
        ));
        assert!(
            found.iter().any(|d| d.contains("never runs")),
            "`if` outside a tool event silently disables the handler: {found:?}"
        );
        // On a tool event the same field is correct and must pass clean.
        assert!(
            validate_hooks(&one_handler(
                "PreToolUse",
                command_group(Some("Bash"), json!({"if": "Bash(git *)"})),
            ))
            .is_empty(),
            "`if` is exactly right on a tool event"
        );
    }

    #[test]
    fn a_matcher_value_outside_a_closed_set_is_refused() {
        // `startupp` can never fire; the whole group is dead.
        let found = diagnose(one_handler(
            "SessionStart",
            command_group(Some("startup|startupp"), json!({})),
        ));
        assert!(
            found
                .iter()
                .any(|d| d.contains("\"startupp\"") && d.contains("never fire")),
            "{found:?}"
        );
        // Every documented value must be accepted, or the check is a trap of
        // its own.
        for event in HOOK_EVENTS {
            let Some(values) = enumerated_matcher_values(event) else {
                continue;
            };
            let matcher = values.join("|");
            let found = diagnose(one_handler(event, command_group(Some(&matcher), json!({}))));
            assert!(
                found.is_empty(),
                "{event} must accept its own documented values {matcher:?}: {found:?}"
            );
        }
    }

    #[test]
    fn a_narrow_matcher_event_warns_when_a_hyphen_drops_it_onto_the_regex_path() {
        let found = diagnose(one_handler(
            "FileChanged",
            command_group(Some(".env|.envrc"), json!({})),
        ));
        assert!(
            found
                .iter()
                .any(|d| d.starts_with("warning") && d.contains("regular-expression path")),
            "a dot is outside FileChanged's exact set: {found:?}"
        );
    }

    #[test]
    fn a_bidi_character_anywhere_is_refused() {
        // U+202E reverses the rendering of everything after it, so what a
        // reviewer reads is not what the matcher matches.
        let found = diagnose(one_handler(
            "PreToolUse",
            command_group(Some("Bash\u{202E}"), json!({})),
        ));
        assert!(
            found.iter().any(|d| d.contains("bidirectional")),
            "{found:?}"
        );
        let manifest = validate_plugin_manifest(&json!({"name": "dev\u{202E}map"}));
        assert!(
            manifest
                .iter()
                .any(|d| d.to_string().contains("bidirectional")),
            "{manifest:?}"
        );
    }

    /// R2: validation rejects bidi controls only. ZWNJ/ZWJ are ordinary in
    /// Persian, Indic text and emoji, so a name carrying one is not refused
    /// (display sinks escape them instead).
    #[test]
    fn zero_width_joiners_are_not_refused_but_every_bidi_control_is() {
        let ok = validate_plugin_manifest(&json!({"name": "می\u{200C}خواهم\u{200D}"}));
        assert!(
            !ok.iter().any(|d| d.to_string().contains("bidirectional")),
            "{ok:?}"
        );
        for c in [
            '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}',
        ] {
            let bad = validate_plugin_manifest(&json!({ "name": format!("dev{c}map") }));
            assert!(
                bad.iter().any(|d| d.to_string().contains("bidirectional")),
                "U+{:04X}: {bad:?}",
                c as u32
            );
        }
    }

    #[test]
    fn handler_shape_is_checked_per_type() {
        for (handler, needle) in [
            (json!({"type": "command"}), "requires `command`"),
            (json!({"type": "http"}), "requires `url`"),
            (
                json!({"type": "mcp_tool", "server": "s"}),
                "requires `tool`",
            ),
            (json!({"type": "prompt"}), "requires `prompt`"),
            (
                json!({"type": "webhook", "url": "u"}),
                "not a hook handler type",
            ),
            (json!({"command": "/bin/true"}), "no `type`"),
            (
                json!({"type": "command", "command": "x", "args": "not-an-array"}),
                "`args` is refused",
            ),
            (
                json!({"type": "command", "command": "/abs/devmap hook session-start", "args": ["build"]}),
                "`args` is refused",
            ),
            (
                json!({"type": "command", "command": "x", "shell": "zsh"}),
                "\"bash\" or \"powershell\"",
            ),
        ] {
            let found = diagnose(json!({"hooks": {"Stop": [{"hooks": [handler]}]}}));
            assert!(
                found.iter().any(|d| d.contains(needle)),
                "expected {needle:?} in {found:?}"
            );
        }
    }

    #[test]
    fn an_empty_or_missing_handler_list_is_refused() {
        for group in [json!({"matcher": ""}), json!({"matcher": "", "hooks": []})] {
            let found = diagnose(json!({"hooks": {"Stop": [group]}}));
            assert!(
                found.iter().any(|d| d.contains("hooks")),
                "a group that runs nothing is not a configured hook: {found:?}"
            );
        }
    }

    #[test]
    fn a_manifest_without_a_name_is_refused_and_path_fields_must_be_relative() {
        let found: Vec<String> = validate_plugin_manifest(&json!({"description": "x"}))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(
            found
                .iter()
                .any(|d| d.contains("one required manifest field")),
            "{found:?}"
        );

        let found: Vec<String> = validate_plugin_manifest(&json!({
            "name": "devmap",
            "hooks": "hooks/hooks.json",
            "agents": ["/abs/agents"],
            "keywords": "not-an-array",
            "defaultEnabled": "yes",
            "author": "DevCouncil",
            "metadata": 7,
            "surprise": true,
        }))
        .iter()
        .map(ToString::to_string)
        .collect();
        for needle in [
            "`hooks` path",
            "`agents` path",
            "`keywords` must be an array",
            "`defaultEnabled` must be a boolean",
            "`author` must be an object",
        ] {
            assert!(
                found
                    .iter()
                    .any(|d| d.starts_with("error") && d.contains(needle)),
                "expected error {needle:?} in {found:?}"
            );
        }
        // Documented as ignored-with-a-warning, not a load failure. Reporting
        // these as errors would refuse a manifest Claude Code accepts.
        for needle in [
            "`metadata` must be an object",
            "not a recognized manifest field",
        ] {
            assert!(
                found
                    .iter()
                    .any(|d| d.starts_with("warning") && d.contains(needle)),
                "expected warning {needle:?} in {found:?}"
            );
        }
        // `skills` alone may name the plugin root.
        assert!(
            validate_plugin_manifest(&json!({"name": "devmap", "skills": "."})).is_empty(),
            "`skills: \".\"` is documented as valid"
        );
        assert!(
            validate_plugin_manifest(&json!({"name": "devmap", "commands": "."}))
                .iter()
                .any(|d| d.severity == Severity::Error),
            "only `skills` accepts a bare \".\""
        );
    }

    #[test]
    fn a_reserved_marketplace_name_is_refused() {
        let found: Vec<String> = validate_marketplace(&json!({
            "name": "anthropic-plugins",
            "owner": {"name": "someone"},
            "plugins": [{"name": "devmap", "source": "./devmap"}],
        }))
        .iter()
        .map(ToString::to_string)
        .collect();
        assert!(
            found.iter().any(|d| d.contains("reserved")),
            "a reserved name fails as an untrusted source, which reads as a broken \
             install rather than a bad name: {found:?}"
        );
    }

    #[test]
    fn a_marketplace_missing_a_required_field_is_refused() {
        for (document, needle) in [
            (
                json!({"owner": {"name": "x"}, "plugins": []}),
                "`name` is required",
            ),
            (json!({"name": "m", "plugins": []}), "`owner` is required"),
            (
                json!({"name": "m", "owner": {"name": "x"}}),
                "`plugins` is required",
            ),
            (
                json!({"name": "m", "owner": {}, "plugins": []}),
                "`owner.name` is required",
            ),
            (
                json!({"name": "m", "owner": {"name": "x"}, "plugins": [{"name": "p"}]}),
                "requires `source`",
            ),
        ] {
            let found: Vec<String> = validate_marketplace(&document)
                .iter()
                .map(ToString::to_string)
                .collect();
            assert!(
                found.iter().any(|d| d.contains(needle)),
                "expected {needle:?} in {found:?}"
            );
        }
    }

    /// Strict mode must actually change the verdict, or `--strict` is decoration.
    #[test]
    fn strict_mode_promotes_warnings_to_a_failing_verdict() {
        let warned = validate_plugin_manifest(&json!({"name": "devmap", "surprise": 1}));
        assert!(Report::new(warned.clone(), false).ok());
        assert!(!Report::new(warned, true).ok());
    }

    /// `--http 8080` must mean this machine, not every interface.
    ///
    /// A Dev Map store is a symbol-level map of a private repository. Binding
    /// it to a routable interface publishes that map to anything that can reach
    /// the port, with no authentication in front of it. `mcp_http.rs` guards
    /// against DNS rebinding *onto a loopback listener*, but nothing can
    /// un-publish a listener bound to `0.0.0.0` in the first place.
    ///
    /// `normalize_http_address` decides that and had no test of any kind: it
    /// appeared twice in the tree, both times in production code. A default
    /// exercised only by running the binary is one a refactor can simplify away
    /// with nothing failing.
    #[test]
    fn a_bare_port_binds_loopback_and_an_ambiguous_address_binds_nothing() {
        // What the CLI does end to end: normalize, then parse before binding.
        let resolve = |address: &str| -> Result<std::net::SocketAddr, String> {
            normalize_http_address(address)
                .parse()
                .map_err(|e| format!("{e}"))
        };

        for port in ["1", "80", "8080", "65535"] {
            let addr = resolve(port).unwrap_or_else(|e| panic!("bare port {port}: {e}"));
            assert!(
                addr.ip().is_loopback(),
                "--http {port} resolved to {addr}, which is not loopback. A bare port \
                 is what a user types without thinking about interfaces, and this \
                 store is a map of a private repository."
            );
            assert_eq!(addr.port().to_string(), port, "port changed: {addr}");
        }

        // An explicit routable bind stays allowed — deliberate is spelled with
        // an address. Rewriting it to loopback would make it appear to work
        // somewhere the caller did not ask for.
        let explicit = resolve("0.0.0.0:8080").expect("an explicit address resolves");
        assert!(
            !explicit.ip().is_loopback(),
            "0.0.0.0 was rewritten to loopback"
        );

        assert!(
            resolve("[::1]:8080")
                .expect("bracketed IPv6")
                .ip()
                .is_loopback(),
            "[::1] is loopback"
        );

        // Ambiguous forms carry a colon, so they bypass the loopback default.
        // Each must fail at the parse rather than bind a guess.
        for ambiguous in [":8080", "::1", "", "8080:", "*:8080", "localhost:8080"] {
            assert!(
                resolve(ambiguous).is_err(),
                "'{ambiguous}' resolved to {:?} instead of being refused; an address \
                 the user did not clearly specify must not be guessed into a bind",
                resolve(ambiguous)
            );
        }
    }

    /// Shell calls reach the index: `Bash` is on the PostToolUse matcher Claude
    /// Code and Codex both read, spelled exactly (a regex would be unchecked),
    /// every alternative is a documented tool, and nothing moved onto a
    /// permission event.
    #[test]
    fn the_post_tool_use_matcher_covers_shell_calls_and_stays_off_permission_events() {
        let block = hooks_block(
            &absolute_fixture("/usr/local/bin/devmap"),
            &["hook".to_string()],
        )
        .expect("normal emission");
        let matcher = block["hooks"]["PostToolUse"][0]["matcher"]
            .as_str()
            .unwrap();
        assert_eq!(matcher, "Edit|Write|NotebookEdit|Bash");
        let MatcherKind::Exact(tools) = classify_matcher("PostToolUse", matcher) else {
            panic!("{matcher:?} must be an exact list, not a regular expression");
        };
        assert!(tools.iter().any(|tool| tool == "Bash"), "{tools:?}");
        for tool in &tools {
            assert!(
                BUILTIN_TOOLS.contains(&tool.as_str()),
                "{tool} is not a documented tool"
            );
        }
        for event in PERMISSION_DECIDING_EVENTS {
            assert!(
                block["hooks"].get(*event).is_none(),
                "{event} must stay unhandled"
            );
        }
        assert_ne!(matcher, "Edit|Write|NotebookEdit", "the pre-shell matcher");
    }

    /// A shell command that wrote and then failed (a merge stopped on conflicts)
    /// fires PostToolUseFailure: it is routed to the post-tool-use hook for
    /// Bash only, on a non-permission event, and its coverage row says handled.
    #[test]
    fn a_failed_shell_call_reaches_the_post_tool_use_hook() {
        let block = hooks_block(
            &absolute_fixture("/usr/local/bin/devmap"),
            &["hook".to_string()],
        )
        .expect("normal emission");
        let group = &block["hooks"]["PostToolUseFailure"][0];
        assert_eq!(group["matcher"], "Bash");
        let command = group["hooks"][0]["command"].as_str().unwrap();
        assert!(command.ends_with(" hook post-tool-use"), "{command}");
        assert!(!PERMISSION_DECIDING_EVENTS.contains(&"PostToolUseFailure"));
        let (_, hook, reason) = event_coverage()
            .into_iter()
            .find(|(event, _, _)| *event == "PostToolUseFailure")
            .unwrap();
        assert!(hook.is_some(), "{reason}");
        assert!(reason.contains("PostToolUseFailure"), "{reason}");
    }

    /// Cursor's shell refresh is a `postToolUse` entry with the exact matcher
    /// `Shell`, run as `hook post-tool-use`. Cursor documents `Shell` as the
    /// terminal tool's matcher value and sends `cwd` plus `tool_input.command`
    /// on `postToolUse` (https://cursor.com/docs/agent/hooks, read 2026-11-21),
    /// which are the fields the hook reads.
    #[test]
    fn cursor_shell_refresh_is_a_post_tool_use_entry_for_shell() {
        let exe = absolute_fixture("/usr/local/bin/devmap");
        let doc = cursor_hooks_document(&exe).unwrap();
        let entries = doc["hooks"]["postToolUse"].as_array().unwrap();
        let shell: Vec<_> = entries
            .iter()
            .filter(|entry| entry["matcher"] == "Shell")
            .collect();
        assert_eq!(shell.len(), 1, "{entries:?}");
        assert_eq!(
            shell[0]["command"],
            format!("'{}' hook post-tool-use", exe.display())
        );
        assert!(doc["hooks"].get("afterShellExecution").is_none());
    }

    /// Codex documents its hook events and `PostToolUseFailure` is not one of
    /// them (https://learn.chatgpt.com/docs/hooks, read 2026-11-21), so neither
    /// Codex `hooks.json` carries it; Claude's still does.
    #[test]
    fn codex_hooks_json_omits_events_codex_does_not_document() {
        let files = render_plugin_bundle(
            &absolute_fixture("/usr/local/bin/devmap"),
            Path::new("/tmp/devmap.sqlite"),
            Some("0.0.0"),
            &["hook".to_string()],
        )
        .unwrap();
        let mut codex = 0;
        for (path, text) in files
            .iter()
            .filter(|(path, _)| path.file_name().is_some_and(|name| name == "hooks.json"))
        {
            let document: Value = serde_json::from_str(text).unwrap();
            let is_codex = path
                .components()
                .any(|part| part.as_os_str() == ".codex-plugin");
            if is_codex {
                codex += 1;
                assert!(document["hooks"].get("PostToolUseFailure").is_none());
                assert!(document["hooks"]["PostToolUse"].is_array());
                for event in document["hooks"].as_object().unwrap().keys() {
                    assert!(
                        CODEX_DOCUMENTED_EVENTS.contains(&event.as_str()),
                        "Codex hooks.json carries {event}, which Codex does not document"
                    );
                }
            } else {
                assert!(document["hooks"]["PostToolUseFailure"].is_array());
            }
        }
        assert_eq!(codex, 1);
    }

    /// Both bundles that carry a hooks.json (Claude's and Codex's) are rendered
    /// from the one table, so both pick up the shell matcher.
    #[test]
    fn every_rendered_hooks_json_carries_the_shell_matcher() {
        let files = render_plugin_bundle(
            &absolute_fixture("/usr/local/bin/devmap"),
            Path::new("/tmp/devmap.sqlite"),
            Some("0.0.0"),
            &["hook".to_string()],
        )
        .expect("bundle renders");
        let hooks: Vec<_> = files
            .iter()
            .filter(|(path, _)| path.file_name().is_some_and(|name| name == "hooks.json"))
            .collect();
        assert_eq!(hooks.len(), 2, "Claude and Codex each ship a hooks.json");
        for (path, text) in hooks {
            let document: Value = serde_json::from_str(text).unwrap();
            let matcher = document["hooks"]["PostToolUse"][0]["matcher"]
                .as_str()
                .unwrap();
            assert!(
                matcher.split('|').any(|tool| tool == "Bash"),
                "{}: {matcher}",
                path.display()
            );
        }
    }

    /// A host runs the emitted command through a shell. Double quotes still
    /// expand `$(...)`; the marker is the proof, not the quote character.
    #[cfg(unix)]
    #[test]
    fn hooks_block_single_quotes_a_path_the_shell_would_expand() {
        let marker = std::env::temp_dir().join(format!(
            "devmap-hook-quote-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&marker);
        let dir = marker.with_extension("dir");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = PathBuf::from("/tmp/$(touch \"$MARKER\")/devmap");
        let block = hooks_block(&exe, &["hook".to_string()]).expect("normal emission");
        let mut commands = Vec::new();
        for groups in block["hooks"].as_object().unwrap().values() {
            for group in groups.as_array().unwrap() {
                for handler in group["hooks"].as_array().unwrap() {
                    commands.push(handler["command"].as_str().unwrap().to_string());
                }
            }
        }
        assert!(!commands.is_empty(), "hooks_block emitted no commands");
        for command in &commands {
            assert!(
                command.starts_with('\''),
                "executable must be single-quoted: {command}"
            );
            assert!(
                command.contains("$(touch \"$MARKER\")"),
                "the metacharacters must survive as literal text: {command}"
            );
            let status = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(command)
                .current_dir(&dir)
                .env("MARKER", &marker)
                .stderr(std::process::Stdio::null())
                .status()
                .expect("sh runs");
            assert!(
                !marker.exists(),
                "shell expanded the executable path ({status}): {command}"
            );
        }
        let ordinary_exe = absolute_fixture("/usr/local/bin/devmap");
        let ordinary = hooks_block(&ordinary_exe, &["hook".to_string()])
            .expect("an absolute path with no shell metacharacters must validate");
        let sample = ordinary["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            sample,
            format!("'{}' hook session-start", ordinary_exe.display())
        );
        let cursor = cursor_hooks_document(&exe).expect("cursor hooks use the same quoting");
        let cursor_cmd = cursor["hooks"]["sessionStart"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            cursor_cmd,
            "'/tmp/$(touch \"$MARKER\")/devmap' hook session-start"
        );
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(cursor_cmd)
            .current_dir(&dir)
            .env("MARKER", &marker)
            .stderr(std::process::Stdio::null())
            .status()
            .expect("sh runs");
        assert!(
            !marker.exists(),
            "cursor hook command expanded the executable path ({status}): {cursor_cmd}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn hooks_block_refuses_a_path_that_single_quotes_cannot_hold() {
        for path in ["/tmp/a\nb/devmap", "/tmp/a\rb/devmap", "/tmp/a\0b/devmap"] {
            let path = absolute_fixture(path);
            let err = hooks_block(&path, &["hook".into()])
                .expect_err("newline, CR, or NUL must not be emitted");
            let text = err.to_string();
            assert!(
                text.contains("newline") && text.contains("NUL"),
                "{path:?} -> {text}"
            );
            assert!(
                !text.contains("absolute"),
                "{path:?} must be refused for its bytes, not for being relative: {text}"
            );
        }
    }

    /// Apostrophe, backticks, and a substitution glued on after a quote must
    /// stay one literal word under `/bin/sh -c`. The marker is the proof.
    #[cfg(unix)]
    #[test]
    fn hooks_block_single_quotes_hold_past_apostrophe_and_backtick_breakouts() {
        let marker = std::env::temp_dir().join(format!(
            "devmap-hook-break-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&marker);
        let dir = marker.with_extension("dir");
        std::fs::create_dir_all(&dir).unwrap();
        let subst = "$(touch \"$MARKER\")";
        let tick = "`touch \"$MARKER\"`";
        // Every case is an absolute path: hooks are never emitted bare (ba667f3f),
        // and the hostile text is what each one still carries.
        let paths = [
            format!("/tmp/{subst}/devmap"),
            format!("/tmp/{tick}/devmap"),
            format!("/tmp/o'{subst}/devmap"),
            format!("/tmp/'{subst}'/devmap"),
            "/tmp/foo'; touch \"$MARKER\"; echo '/devmap".to_string(),
            format!("/tmp/'{subst}"),
            format!("/tmp/{subst}'"),
            format!("/tmp/my bin/{subst}/devmap"),
            format!("/tmp/δé/{subst}/devmap"),
            format!("/tmp/路径/{tick}/devmap"),
            format!("/tmp/$'{subst}'/devmap"),
            format!("/tmp/foo\\ bar/{subst}/devmap"),
            format!("/tmp/a\u{2028}{subst}/devmap"),
            format!("/tmp/a\u{202e}{tick}/devmap"),
            format!("/tmp/foo # {subst}"),
            format!("/tmp/<({subst})/devmap"),
            format!("/tmp/a'b'c'{subst}"),
        ];
        for path in &paths {
            let block = hooks_block(Path::new(path), &["hook".to_string()])
                .unwrap_or_else(|err| panic!("emission panicked or refused {path:?}: {err}"));
            let mut commands = Vec::new();
            for groups in block["hooks"].as_object().unwrap().values() {
                for group in groups.as_array().unwrap() {
                    for handler in group["hooks"].as_array().unwrap() {
                        commands.push(handler["command"].as_str().unwrap().to_string());
                    }
                }
            }
            assert!(!commands.is_empty(), "{path:?} emitted no commands");
            for command in &commands {
                assert!(
                    command.starts_with('\''),
                    "executable must be single-quoted: {command}"
                );
                assert!(
                    !command.contains('\n') && !command.contains('\r') && !command.contains('\0'),
                    "refused bytes were emitted: {command:?}"
                );
                if path.contains('\'') {
                    assert!(
                        command.contains("'\\''"),
                        "an apostrophe must be '\\'' : {command}"
                    );
                }
                let status = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(&dir)
                    .env_clear()
                    .env("MARKER", &marker)
                    .env("PATH", "/usr/bin:/bin")
                    .env("IFS", " \t\n'\\$`;|&<>")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .expect("sh runs");
                assert!(
                    !marker.exists(),
                    "shell expanded the executable path ({status}) from {path:?}: {command}"
                );
            }
        }
        let quoted = hooks_block(
            Path::new("/tmp/o'$(touch \"$MARKER\")/devmap"),
            &["hook".to_string()],
        )
        .expect("apostrophe path emits");
        let sample = quoted["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            sample,
            "'/tmp/o'\\''$(touch \"$MARKER\")/devmap' hook session-start"
        );
        let cursor = cursor_hooks_document(Path::new("/tmp/`touch \"$MARKER\"`/devmap"))
            .expect("cursor hooks use the same quoting");
        let cursor_cmd = cursor["hooks"]["sessionStart"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            cursor_cmd,
            "'/tmp/`touch \"$MARKER\"`/devmap' hook session-start"
        );
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(cursor_cmd)
            .current_dir(&dir)
            .env_clear()
            .env("MARKER", &marker)
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("sh runs");
        assert!(
            !marker.exists(),
            "cursor hook command expanded the executable path ({status}): {cursor_cmd}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&marker);
    }

    /// A newline glued to a substitution is still a newline: refuse, do not emit.
    #[test]
    fn hooks_block_refuses_a_breakout_that_embeds_a_newline_cr_or_nul() {
        for path in [
            "/tmp/ok/devmap\n/bin/touch",
            "/tmp/`touch x`\r/devmap",
            "/tmp/o'$(touch x)\0/devmap",
        ] {
            let path = absolute_fixture(path);
            let err = hooks_block(&path, &["hook".into()])
                .expect_err(&format!("emitted a command for {path:?}"));
            assert!(
                err.to_string().contains("newline"),
                "{path:?} must be refused for its bytes: {err}"
            );
        }
    }

    /// The URL an agent is handed must be the address the server binds.
    ///
    /// `mcp_entry` writes `http://{normalize_http_address(..)}` into the client
    /// config while the serve path binds `normalize_http_address(..)`. One
    /// owner, so they cannot disagree; this pins that they still share it.
    #[test]
    fn the_url_an_agent_is_given_is_the_address_the_server_binds() {
        let executable = &absolute_fixture("/usr/local/bin/devmap");
        let db = Path::new("/repo/.devcouncil/codeintel/devmap.sqlite");
        for address in ["8080", "127.0.0.1:9000", "[::1]:9100"] {
            let entry = mcp_entry(executable, Some(db), Some(address)).expect("http entry renders");
            let url = entry["url"]
                .as_str()
                .unwrap_or_else(|| panic!("the http entry must carry a url: {entry}"));
            let bound = normalize_http_address(address);
            assert_eq!(
                url,
                format!("http://{bound}"),
                "the agent is told to reach {url} while the server binds {bound}"
            );
            if !address.contains(':') {
                assert!(
                    url.starts_with("http://127.0.0.1:"),
                    "a bare port put {url} into an agent's config"
                );
            }
        }
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args([
                "-c",
                "protocol.file.allow=always",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devmap-wt-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let canonical = dir.canonicalize().expect("canonical scratch dir");
        // Windows canonicalizes to a verbatim `\\?\C:\...` path, which git
        // refuses as a worktree or submodule location; the drive form names
        // the same directory.
        match canonical
            .to_str()
            .and_then(|text| text.strip_prefix(r"\\?\"))
        {
            Some(rest) => PathBuf::from(rest),
            None => canonical,
        }
    }

    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("repo dir");
        git_in(dir, &["init", "-q"]);
        git_in(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }

    /// Build output in a worktree git registered warns and is still accepted:
    /// `CARGO_BIN_EXE_devmap` is exactly that path when the suite runs in a lane.
    #[test]
    fn a_registered_linked_worktrees_build_output_warns_and_is_accepted() {
        let root = scratch_dir("lane");
        let main = root.join("main");
        init_repo(&main);
        let lane = root.join("lane");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                lane.to_str().expect("utf8"),
                "-b",
                "lane",
            ],
        );
        let primary_bin = main.join("rust/target/debug/devmap");
        let cases = [
            lane.join("rust/target/debug/devmap"),
            lane.join("rust/target/aarch64-apple-darwin/release/devmap"),
            lane.join("rust/target/custom-profile/devmap"),
            lane.join("target/x/target/ci/devmap"),
        ];
        let warned: Vec<_> = cases
            .iter()
            .map(|p| (p.clone(), worktree_build_output_warning(p)))
            .collect();
        let accepted: Vec<_> = cases.iter().map(|p| mcp_entry(p, None, None)).collect();
        let primary = worktree_build_output_warning(&primary_bin);
        let note_direct = mcp_receipt_note(&lane.join("rust/target/debug/devmap"), true);
        let note_unregistered = mcp_receipt_note(&lane.join("rust/target/debug/devmap"), false);
        // A path no writer above has used, so the set has not seen it yet.
        let unseen = lane.join("rust/target/debug/unseen-devmap");
        let first = first_sighting(&unseen);
        let second = first_sighting(&unseen);
        let source_file = worktree_build_output_warning(&lane.join("rust/src/target"));
        let _ = std::fs::remove_dir_all(&root);
        for (path, warning) in warned {
            let warning = warning.unwrap_or_else(|| panic!("{} was not flagged", path.display()));
            assert!(warning.contains("removed"), "{warning}");
            assert!(warning.contains("--binary"), "{warning}");
            assert!(warning.contains(&*path.to_string_lossy()), "{warning}");
        }
        for entry in accepted {
            entry.expect("a warning is not a refusal");
        }
        assert!(first && !second, "stderr warns once per path");
        assert!(note_direct.is_some(), "receipt note missing");
        assert!(
            note_unregistered.is_none(),
            "nothing registered, nothing will break"
        );
        assert!(
            primary.is_none(),
            "the main checkout outlives any lane: {primary:?}"
        );
        assert!(
            source_file.is_none(),
            "a file named target is not build output"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_a_lanes_build_output_warns_and_notes() {
        let root = scratch_dir("link");
        let main = root.join("main");
        init_repo(&main);
        let lane = root.join("lane");
        git_in(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                lane.to_str().expect("utf8"),
                "-b",
                "lane",
            ],
        );
        let binary = lane.join("rust/target/debug/devmap");
        std::fs::create_dir_all(binary.parent().expect("parent")).expect("lane target");
        std::fs::write(&binary, b"x").expect("lane binary");
        let link = root.join("devmap");
        std::os::unix::fs::symlink(&binary, &link).expect("symlink");
        let warning = worktree_build_output_warning(&link);
        let note = mcp_receipt_note(&link, true);
        let _ = std::fs::remove_dir_all(&root);
        assert!(warning.is_some(), "a symlink into a lane was not flagged");
        assert!(
            note.is_some(),
            "the receipt note is missing for the symlink"
        );
    }

    #[test]
    fn a_submodules_target_and_a_plain_worktrees_folder_are_not_flagged() {
        let root = scratch_dir("neg");
        let sub_src = root.join("sub-src");
        init_repo(&sub_src);
        let main = root.join("main");
        init_repo(&main);
        git_in(
            &main,
            &[
                "submodule",
                "add",
                "-q",
                sub_src.to_str().expect("utf8"),
                "vendor/sub",
            ],
        );
        let submodule_bin = main.join("vendor/sub/target/debug/devmap");
        let outside_git = root.join("not-a-repo/worktrees/lane/rust/target/debug/devmap");
        let flagged = [
            worktree_build_output_warning(&submodule_bin),
            worktree_build_output_warning(&outside_git),
        ];
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(flagged, [None, None]);
    }

    /// The ENOENT from 2026-10-29 was this path: a hashed copy under
    /// `target/devcouncil-testbin` inside a worktree that was later deleted.
    /// Registering it is the failure. A checkout's own `target/debug` or
    /// `target/release` binary stays a valid absolute command, because
    /// `mcp --print-config` is specified to name the binary that ran.
    #[cfg(unix)]
    #[test]
    fn a_hashed_test_binary_is_refused_as_an_mcp_command() {
        let stale = Path::new(
            "/Users/bharath/Code/devtools/DevCouncil-verify-staged/rust/target/devcouncil-testbin/devmap-73b18311b815cc4f/devmap",
        );
        let err = mcp_entry(stale, None, None)
            .expect_err("a devcouncil-testbin path must not become an MCP command");
        let text = err.to_string();
        assert!(
            text.contains("devcouncil-testbin"),
            "the refusal must name the directory: {text}"
        );
        assert!(
            text.contains("73b18311b815cc4f"),
            "the refusal must name the path that would have been spawned: {text}"
        );

        let rooted = mcp_entry_with_root(stale, Path::new("/repo"), None)
            .expect_err("the --root form writes the same command");
        assert!(
            rooted.to_string().contains("devcouncil-testbin"),
            "{rooted}"
        );

        let http = mcp_entry(stale, None, Some("8080"))
            .expect_err("the http form must not point a client at the test copy");
        assert!(http.to_string().contains("devcouncil-testbin"), "{http}");

        // The directory is one path component. A longer name is a different directory.
        for allowed in [
            "/work/rust/target/debug/devmap",
            "/work/rust/target/release/devmap",
            "/opt/devcouncil-testbin-backup/devmap",
            "/Users/x/.local/bin/devmap",
            "/tmp/帮助/devmap",
        ] {
            let entry = mcp_entry(Path::new(allowed), None, None)
                .unwrap_or_else(|err| panic!("{allowed} was refused: {err}"));
            assert_eq!(entry["command"], allowed, "{entry}");
            assert_eq!(entry["args"], json!(["mcp"]));
        }

        // A symlink whose own path is innocent still resolves into the test dir.
        let root = std::env::temp_dir().join(format!(
            "devmap-mcp-guard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let bin_dir = root.join("devcouncil-testbin").join("devmap-abc");
        std::fs::create_dir_all(&bin_dir).expect("test bin dir");
        let bin = bin_dir.join("devmap");
        std::fs::write(&bin, b"not a real binary").expect("test bin");
        let link_dir = root.join("elsewhere");
        std::fs::create_dir_all(&link_dir).expect("link dir");
        let link = link_dir.join("devmap");
        std::os::unix::fs::symlink(&bin, &link).expect("symlink");
        let linked = mcp_entry(&link, None, None);
        let _ = std::fs::remove_dir_all(&root);
        let linked = linked.expect_err("a symlink into devcouncil-testbin is still that binary");
        assert!(
            linked.to_string().contains("devcouncil-testbin"),
            "{linked}"
        );
    }

    #[test]
    fn a_relative_executable_is_never_registered() {
        for relative in ["devmap", "./devmap", "bin/devmap"] {
            let path = Path::new(relative);
            assert!(absolute_executable(path).is_err(), "{relative}");
            assert!(
                hooks_block(path, &["hook".to_string()]).is_err(),
                "{relative}"
            );
            assert!(cursor_hooks_document(path).is_err(), "{relative}");
            assert!(mcp_entry(path, None, None).is_err(), "{relative}");
            assert!(
                mcp_entry_with_root(path, Path::new("/repo"), None).is_err(),
                "{relative}"
            );
        }
        let installed = absolute_fixture("/usr/local/bin/devmap");
        assert_eq!(absolute_executable(&installed).unwrap(), installed);
    }

    #[test]
    fn one_predicate_judges_command_words() {
        let home = Path::new("/home/u");
        let verdict = |word: &str| judge_command_word(word, Some(home), None).unwrap();
        for relative in ["devmap", "./devmap", "bin/devmap", "../x/devmap"] {
            assert_eq!(verdict(relative), WordVerdict::Relative, "{relative}");
        }
        for expanded in [
            "$CLAUDE_PROJECT_DIR/bin/devmap",
            "${CLAUDE_PROJECT_DIR}/devmap",
            "$HOMEX/devmap",
            "~other/bin/devmap",
        ] {
            assert_eq!(verdict(expanded), WordVerdict::Expanded, "{expanded}");
        }
        let usr_bin = absolute_fixture_slashed("/usr/bin/devmap");
        for fine in [
            usr_bin.as_str(),
            "~/.local/bin/devmap",
            "$HOME/.local/bin/devmap",
            "${HOME}/.local/bin/devmap",
        ] {
            assert_eq!(verdict(fine), WordVerdict::Fine, "{fine}");
        }
        assert!(judge_command_word("~/devmap", None, None).is_err());
        assert!(command_word_defect("'$CLAUDE_PROJECT_DIR'/devmap hook x").is_some());
        assert!(command_word_defect(&format!("'{usr_bin}' hook x")).is_none());
        assert!(command_word_defect("devmap hook x").is_some());
    }
}
