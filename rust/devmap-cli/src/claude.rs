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

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

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
                         `\"<abs>\" hook <event>` instead",
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
                    let trimmed = command.trim();
                    // A bare name with no path separator is the Cursor failure mode.
                    if !trimmed.contains('/') && !trimmed.contains('\\') && !trimmed.contains(' ') {
                        out.push(Diagnostic::error(
                            at,
                            format!(
                                "hook `command` {trimmed:?} is a bare name; Cursor/Codex need an \
                                 absolute-path shell-form command (\"/path/to/devmap\" hook <event>)"
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

/// Validate a `plugin.json` document against the documented manifest schema.
pub fn validate_plugin_manifest(document: &Value) -> Vec<Diagnostic> {
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
        if !MANIFEST_KEYS.contains(&key.as_str()) {
            out.push(Diagnostic::warning(
                key,
                format!("{key:?} is not a recognized manifest field; Claude Code ignores it"),
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
pub const MARKETPLACE_NAME: &str = "devmap-local";
pub const MCP_SERVER_NAME: &str = "devmap";
const PLUGIN_HOMEPAGE: &str = "https://github.com/bharathvbcr/DevCouncil";
const PLUGIN_REPOSITORY: &str = "https://github.com/bharathvbcr/DevCouncil.git";
const PLUGIN_LICENSE: &str = "Apache-2.0";

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
/// `Bash` is deliberately absent: a shell command may or may not write, and
/// firing a rebuild after every `ls` would pay for the index on every read.
/// `MultiEdit` is absent because it is not in the tool table — an exact matcher
/// naming it would be a dead alternative dressed as coverage.
pub const WRITE_TOOL_MATCHER: &str = "Edit|Write|NotebookEdit";

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
/// Handlers are shell-form only (`"<abs>" hook <event>`): Cursor drops `args`,
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
        event: "PostToolUse",
        matcher: WRITE_TOOL_MATCHER,
        hook_event: "post-tool-use",
        timeout_secs: Some(5),
        status_message: "",
        purpose: "An incremental rebuild after a write. `devmap hook post-tool-use` \
                  returns within ~100 ms and spawns a detached, coalesced build so \
                  hosts without `async` never block the agent loop.",
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
                "UserPromptSubmit" | "UserPromptExpansion" | "MessageDisplay" => {
                    "Not handled: these run on the human's critical path, and index status \
                     on every prompt is noise rather than context."
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
                "PostToolUseFailure" => "Not handled: a tool that failed wrote nothing to index.",
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
fn utf8_path(label: &str, path: &Path) -> anyhow::Result<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        anyhow::anyhow!(
            "{label} is not valid UTF-8 ({}), and JSON cannot carry it without corrupting \
             the path the hook would run",
            path.display()
        )
    })
}

/// Absolute path for a hook `command`, even when PATH resolves to self.
///
/// MCP entries still use [`plugin_command`]'s bare-name rule; hooks must be
/// absolute because Cursor/Codex do not expand a bare name the same way, and a
/// bare `devmap` that clap-rejects exits 2 and blocks the agent.
pub fn hook_executable(executable: &Path) -> PathBuf {
    if executable.is_absolute() {
        return executable.to_path_buf();
    }
    executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf())
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

/// The command string to write into an emitted hook or MCP entry.
///
/// `std::env::current_exe()` is an absolute path into whatever tree the running
/// binary came from — for a local build, `…/rust/target/release/devmap`.
/// Baked into a bundle, that path is wrong in two ordinary situations: a bundle
/// committed to the repository names a path that exists on one machine, and a
/// bundle emitted from a build tree keeps pointing at a stale binary after
/// `cargo install` puts a current one on `PATH`.
///
/// So the bare name `devmap` is emitted when `devmap` on `PATH` *is* this
/// binary, and the absolute path otherwise. The test is by resolved path rather
/// than by name: a *different* `devmap` earlier on `PATH` is precisely the case
/// where the absolute path is the honest answer, because emitting the bare name
/// would silently hand the hook to some other build.
///
/// `explicit` wins over both, for a packager who knows the install location
/// before anything is installed there.
pub fn plugin_command(executable: &Path, explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    let path_var = std::env::var_os("PATH");
    if resolves_to_self(PLUGIN_NAME, executable, path_var.as_deref()) {
        return PathBuf::from(PLUGIN_NAME);
    }
    executable.to_path_buf()
}

/// True when looking `name` up on `path_var` finds `executable` itself.
///
/// Compares canonicalized paths, so a `PATH` entry that is a symlink to the
/// binary — which is what `cargo install` and every package manager produce —
/// counts as the same file rather than as a different one.
fn resolves_to_self(name: &str, executable: &Path, path_var: Option<&std::ffi::OsStr>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    let Ok(target) = executable.canonicalize() else {
        return false;
    };
    for dir in std::env::split_paths(path_var) {
        // An empty `PATH` entry means the working directory on some shells.
        // Resolving a hook command against wherever the agent happens to be
        // running is not a behaviour worth reproducing.
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        let Ok(resolved) = candidate.canonicalize() else {
            continue;
        };
        // The first hit on PATH decides, exactly as execution would. Continuing
        // past it would let a *later* entry that happens to be this binary
        // authorize emitting a bare name that resolves to the earlier one.
        return resolved == target;
    }
    false
}

/// Build the `{"hooks": {...}}` block Dev Map installs.
///
/// `subcommands` is the parser's own list; a missing `hook` subcommand is
/// refused rather than written.
///
/// Shell-form only: `"command": "\"<abs>\" hook <event>"`. No `args`, no
/// `async`, no `${CLAUDE_PROJECT_DIR}` — Cursor drops args, Codex ignores async,
/// and root selection happens inside the hook from stdin.
pub fn hooks_block(executable: &Path, subcommands: &[String]) -> anyhow::Result<Value> {
    if !subcommands.iter().any(|name| name == "hook") {
        anyhow::bail!(
            "this binary does not register `devmap hook`, so the emitted handlers \
             would fail on every fire. Known subcommands: {}",
            subcommands.join(", ")
        );
    }
    let exe = hook_executable(executable);
    let exe_text = utf8_path("the devmap executable path", &exe)?;

    // Groups are keyed by (event, matcher): two handlers that fire on the same
    // event+matcher belong in one group. Two groups with the same matcher are
    // refused by Claude Code as a duplicate (both would fire, twice).
    let mut grouped: BTreeMap<(&str, &str), Vec<Value>> = BTreeMap::new();
    for spec in DEVMAP_HOOKS {
        if PERMISSION_DECIDING_EVENTS.contains(&spec.event) {
            anyhow::bail!(
                "{} decides an authorization outcome. Dev Map does not install hooks on \
                 {}, because a code index has nothing to contribute to a permission \
                 decision and a handler there would widen what it can approve.",
                spec.event,
                PERMISSION_DECIDING_EVENTS.join(", ")
            );
        }
        let command = format!("\"{exe_text}\" hook {}", spec.hook_event);

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
    Ok(Value::Object(manifest))
}

/// Cursor native `.cursor/hooks.json` (version 1, camelCase, flat shell commands).
///
/// Marker-owned via `generatedBy: "devmap"`. `afterFileEdit` covers Write (Tab
/// completions excluded — they are not repository edits Dev Map should index).
pub fn cursor_hooks_document(executable: &Path) -> anyhow::Result<Value> {
    let exe = utf8_path("the devmap executable path", &hook_executable(executable))?;
    let cmd = |event: &str| format!("\"{exe}\" hook {event}");
    Ok(json!({
        "version": 1,
        "generatedBy": "devmap",
        "hooks": {
            "sessionStart": [{ "command": cmd("session-start") }],
            "afterFileEdit": [{ "command": cmd("post-tool-use") }],
            "sessionEnd": [{
                "command": cmd("session-end"),
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
    // Always the absolute path this binary validated. Emitting a bare `devmap`
    // silently hands the host whichever build wins on PATH; two installs with
    // different schemas is the skew doctor reports.
    let command = utf8_path("the devmap executable path", executable)?;
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
    if http.is_some() {
        return mcp_entry(executable, None, http);
    }
    let command = utf8_path("the devmap executable path", executable)?;
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
            plugin.join("hooks").join("hooks.json"),
            pretty(&hooks_block(executable, subcommands)?),
        ),
        (
            plugin.join(".mcp.json"),
            pretty(&plugin_mcp_config(executable, db)?),
        ),
        (
            plugin.join(".codex-plugin").join("plugin.json"),
            pretty(&codex_plugin_manifest(version)?),
        ),
        (
            plugin
                .join(".codex-plugin")
                .join("hooks")
                .join("hooks.json"),
            pretty(&hooks_block(executable, subcommands)?),
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
    let mut written = Vec::with_capacity(rendered.len());
    for (relative, json) in rendered {
        let path = out_dir.join(&relative);
        let changed = devmap_query::write_atomic(&path, json.as_bytes())
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
            out.extend(validate_plugin_manifest(&value));
        }
        out
    } else if value.get("name").is_some() {
        validate_plugin_manifest(&value)
    } else {
        vec![Diagnostic::error(
            "<root>",
            "not a recognizable Claude Code artifact: expected a hook config (`hooks`), a \
             plugin manifest (`name`), or a marketplace (`name` + `owner` + `plugins`)",
        )]
    };
    Ok(Report::new(diagnostics, strict))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut handler = json!({"type": "command", "command": "/bin/true"});
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

    /// The URL an agent is handed must be the address the server binds.
    ///
    /// `mcp_entry` writes `http://{normalize_http_address(..)}` into the client
    /// config while the serve path binds `normalize_http_address(..)`. One
    /// owner, so they cannot disagree; this pins that they still share it.
    #[test]
    fn the_url_an_agent_is_given_is_the_address_the_server_binds() {
        let executable = Path::new("/usr/local/bin/devmap");
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
}
