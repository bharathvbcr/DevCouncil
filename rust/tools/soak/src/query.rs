//! One receipt-admission owner for standalone and IPC soak queries.
//!
//! An OK envelope does not prove an edit was indexed, and an empty page does
//! not prove a symbol is gone. Every reply the soak reads — `devmap --json
//! search` on the build path, the daemon's IPC envelope on the other — is
//! admitted here, so the two modes cannot drift onto different contracts.

use serde_json::{Map, Value};

/// Where the reply came from. The IPC envelope wraps the result in
/// `{"ok": true, "result": …}`; the CLI prints the result itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Ipc,
    Cli,
}

/// What the soak is asking of the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only that the reply is a well-formed result.
    Validate,
    /// That the store reports itself fresh.
    Fresh,
    /// That the named symbol is listed at the named path.
    Present,
    /// That the named symbol is provably not listed at the named path.
    Absent,
}

/// The verdict on one reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// What was asked is observed.
    Observed,
    /// The reply is valid but does not show it yet; ask again.
    NotYet,
    /// The reply cannot be examined: malformed, an error, or a resolution
    /// other than `Available`. Never retried into a pass.
    Unexaminable,
}

/// A non-negative integer count, as the kernel prints them.
fn count(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|f| *f >= 0.0 && f.fract() == 0.0)
                .map(|f| f as u64)
        }),
        Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            s.parse().ok()
        }
        _ => None,
    }
}

/// A scalar a row names a symbol or a path with.
fn scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Admit `reply` for `mode`, looking for `name` defined in `path`.
pub fn admit(transport: Transport, mode: Mode, name: &str, path: &str, reply: &str) -> Observation {
    use Observation::{NotYet, Observed, Unexaminable};
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(reply) else {
        return Unexaminable;
    };
    let result: &Map<String, Value> = match transport {
        Transport::Ipc => {
            if root.get("ok") != Some(&Value::Bool(true)) {
                return Unexaminable;
            }
            match root.get("result") {
                Some(Value::Object(result)) => result,
                _ => return Unexaminable,
            }
        }
        Transport::Cli => &root,
    };
    match mode {
        Mode::Validate => return Observed,
        Mode::Fresh => {
            return match result.get("is_fresh") {
                Some(Value::Bool(true)) => Observed,
                Some(Value::Bool(false)) => NotYet,
                _ => Unexaminable,
            }
        }
        Mode::Present | Mode::Absent => {}
    }
    if result.get("resolution").and_then(Value::as_str) != Some("Available") {
        return Unexaminable;
    }
    let Some(Value::Array(items)) = result.get("items") else {
        return Unexaminable;
    };
    let (Some(total), Some(shown), Some(hidden)) = (
        count(result.get("total")),
        count(result.get("shown")),
        count(result.get("hidden")),
    ) else {
        return Unexaminable;
    };
    let Some(Value::Bool(truncated)) = result.get("truncated") else {
        return Unexaminable;
    };
    let mut found = false;
    for item in items {
        let Value::Object(item) = item else {
            return Unexaminable;
        };
        let (Some(symbol), Some(file)) = (
            scalar(item.get("symbol_name")),
            scalar(item.get("file_path")),
        ) else {
            return Unexaminable;
        };
        found |= symbol == name && file == path;
    }
    if mode == Mode::Present {
        return if found { Observed } else { NotYet };
    }
    // A capped prefix cannot prove absence, even when no returned row matches.
    let listed = items.len() as u64;
    if *truncated || hidden != 0 || shown != listed || total != listed || found {
        return NotYet;
    }
    Observed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use Observation::{NotYet, Observed, Unexaminable};

    fn result(items: Value, changes: Value) -> Value {
        let n = items.as_array().map_or(0, Vec::len);
        let mut base = json!({"items": items, "total": n, "shown": n, "hidden": 0,
                              "truncated": false, "resolution": "Available"});
        for (k, v) in changes.as_object().expect("changes are an object") {
            base[k] = v.clone();
        }
        base
    }

    fn ipc(mode: Mode, result: Value, ok: bool) -> Observation {
        let reply = json!({"ok": ok, "result": result}).to_string();
        admit(Transport::Ipc, mode, "_soak_1", "main.py", &reply)
    }

    fn row(symbol: &str, file: &str) -> Value {
        json!({"symbol_name": symbol, "file_path": file})
    }

    #[test]
    fn an_ok_envelope_without_the_inserted_symbol_is_not_visibility() {
        assert_eq!(
            ipc(Mode::Present, result(json!([]), json!({})), true),
            NotYet
        );
    }

    #[test]
    fn another_file_or_a_prefix_match_is_not_the_inserted_symbol() {
        for item in [row("_soak_1", "other.py"), row("_soak_10", "main.py")] {
            assert_eq!(
                ipc(
                    Mode::Present,
                    result(json!([item.clone()]), json!({})),
                    true
                ),
                NotYet,
                "{item}"
            );
        }
    }

    #[test]
    fn the_exact_symbol_and_file_is_positive_evidence() {
        assert_eq!(
            ipc(
                Mode::Present,
                result(json!([row("_soak_1", "main.py")]), json!({})),
                true
            ),
            Observed
        );
    }

    #[test]
    fn a_removed_symbol_must_really_disappear() {
        assert_eq!(
            ipc(
                Mode::Absent,
                result(json!([row("_soak_1", "main.py")]), json!({})),
                true
            ),
            NotYet
        );
        assert_eq!(
            ipc(Mode::Absent, result(json!([]), json!({})), true),
            Observed
        );
    }

    #[test]
    fn capped_or_inconsistent_results_cannot_prove_absence() {
        for changes in [
            json!({"truncated": true}),
            json!({"hidden": 1, "total": 1}),
            json!({"total": 1}),
            json!({"shown": 1}),
        ] {
            assert_eq!(
                ipc(Mode::Absent, result(json!([]), changes.clone()), true),
                NotYet,
                "{changes}"
            );
        }
    }

    #[test]
    fn unavailable_or_unknown_resolution_cannot_prove_absence_or_presence() {
        for resolution in [json!("Unavailable"), Value::Null, json!("Unknown")] {
            let changes = json!({"resolution": resolution});
            assert_eq!(
                ipc(Mode::Absent, result(json!([]), changes.clone()), true),
                Unexaminable
            );
            assert_eq!(
                ipc(
                    Mode::Present,
                    result(json!([row("_soak_1", "main.py")]), changes.clone()),
                    true
                ),
                Unexaminable,
                "{changes}"
            );
        }
    }

    #[test]
    fn malformed_results_and_server_errors_fail_closed() {
        for bad in [Value::Null, json!({}), json!({"items": null})] {
            assert_eq!(ipc(Mode::Present, bad.clone(), true), Unexaminable, "{bad}");
        }
        assert_eq!(
            ipc(Mode::Present, result(json!([]), json!({})), false),
            Unexaminable
        );
        assert_eq!(
            admit(Transport::Ipc, Mode::Validate, "", "", "not json"),
            Unexaminable
        );
        assert_eq!(
            admit(Transport::Ipc, Mode::Validate, "", "", "[]"),
            Unexaminable
        );
    }

    #[test]
    fn standalone_uses_the_same_observation_contract() {
        for (mode, reply, expected) in [
            (Mode::Present, result(json!([]), json!({})), NotYet),
            (Mode::Absent, result(json!([]), json!({})), Observed),
            (
                Mode::Absent,
                result(json!([]), json!({"truncated": true})),
                NotYet,
            ),
        ] {
            assert_eq!(
                admit(
                    Transport::Cli,
                    mode,
                    "_soak_1",
                    "main.py",
                    &reply.to_string()
                ),
                expected,
                "{reply}"
            );
        }
    }

    #[test]
    fn freshness_must_be_verified() {
        assert_eq!(ipc(Mode::Fresh, json!({"is_fresh": false}), true), NotYet);
        assert_eq!(ipc(Mode::Fresh, json!({"is_fresh": true}), true), Observed);
        assert_eq!(
            ipc(Mode::Fresh, json!({"is_fresh": "yes"}), true),
            Unexaminable
        );
        assert_eq!(ipc(Mode::Fresh, json!({}), true), Unexaminable);
    }

    #[test]
    fn counts_must_be_non_negative_integers() {
        for changes in [
            json!({"total": -1}),
            json!({"shown": "x"}),
            json!({"hidden": 0.5}),
            json!({"total": null}),
        ] {
            assert_eq!(
                ipc(Mode::Absent, result(json!([]), changes.clone()), true),
                Unexaminable,
                "{changes}"
            );
        }
        assert_eq!(
            ipc(Mode::Absent, result(json!([]), json!({"total": 0.0})), true),
            Observed
        );
    }
}
