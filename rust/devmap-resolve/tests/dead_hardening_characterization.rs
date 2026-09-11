//! Characterization of AmbiguousGlobal fan-out from receiver calls.
//!
//! Written against the unmodified resolver: a call with an explicit receiver
//! whose type is not indexed (`String::new()`, `Vec::new()`, untyped `map.get`)
//! used to fall through rung 3 (global bare-name) and emit AmbiguousGlobal
//! edges to every same-named method in the corpus. Measured on GitPulse:
//! 71% of edges were AmbiguousGlobal; `candidate_total` reached 241.
//!
//! Desired: receiver calls never reach the bare-name global rung. External /
//! prelude receivers classify External/Builtin; unknown value receivers stay
//! UninferredReceiver. Bare-name AmbiguousGlobal above the emission ceiling
//! records a ledger row instead of N edges.

use std::path::PathBuf;

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::{Resolution, UnresolvedClass, AMBIGUOUS_FANOUT_CAP};
use devmap_resolve::Resolver;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/fixtures/dead_hardening")
}

fn resolve_sources(files: &[(&str, &str)]) -> devmap_resolve::model::ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions)
}

fn resolve_fixture_dir(relative: &str) -> devmap_resolve::model::ResolutionResult {
    let dir = fixture_root().join(relative);
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "README.md" {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        files.push((name, source));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    resolve_sources(&borrowed)
}

/// The bug: `String::new()` fans out AmbiguousGlobal to every `new` in the
/// corpus. Desired: zero AmbiguousGlobal edges from that call site.
#[test]
fn external_std_receiver_new_does_not_fan_out_ambiguous_global() {
    let result = resolve_fixture_dir("rust_external_new");

    let from_make_string: Vec<_> = result
        .edges
        .iter()
        .filter(|e| {
            e.edge_kind == EdgeKind::Calls
                && e.source_symbol.contains("make_string")
                && e.target_symbol.contains("new")
        })
        .collect();

    assert!(
        from_make_string.iter().all(|e| {
            !matches!(
                e.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )
        }),
        "String::new() must not AmbiguousGlobal-fan-out; got {} edges: {:?}",
        from_make_string.len(),
        from_make_string
            .iter()
            .map(|e| format!(
                "{}->{} {:?}",
                e.source_symbol,
                e.target_symbol,
                e.resolution
                    .as_deref()
                    .map(|r| format!("{r:?}")[..40].to_string())
            ))
            .collect::<Vec<_>>()
    );

    let ambiguous_new = result.edges.iter().filter(|e| {
        e.edge_kind == EdgeKind::Calls
            && matches!(
                e.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )
            && e.target_symbol.ends_with("::new")
            || e.target_symbol == "new"
                && matches!(
                    e.resolution.as_deref(),
                    Some(Resolution::AmbiguousGlobal { .. })
                )
    });
    // Count carefully — avoid operator precedence trap above by re-filtering.
    let ambiguous_new_count = result
        .edges
        .iter()
        .filter(|e| {
            e.edge_kind == EdgeKind::Calls
                && matches!(
                    e.resolution.as_deref(),
                    Some(Resolution::AmbiguousGlobal { .. })
                )
                && (e.target_symbol.ends_with("::new") || e.target_symbol == "new")
        })
        .count();
    let _ = ambiguous_new;
    assert_eq!(
        ambiguous_new_count, 0,
        "no AmbiguousGlobal edges may name `new` when the only callers are std receivers"
    );

    // Positive: the site is still recorded.
    let recorded = result.unresolved.iter().any(|u| {
        u.callee_name == "new"
            && matches!(
                u.class,
                UnresolvedClass::External { .. }
                    | UnresolvedClass::Builtin
                    | UnresolvedClass::UninferredReceiver
            )
    });
    assert!(
        recorded,
        "String::new / Vec::new must land in the ledger as External/Builtin/UninferredReceiver, got {:?}",
        result
            .unresolved
            .iter()
            .filter(|u| u.callee_name == "new")
            .map(|u| format!("{:?}", u.class))
            .collect::<Vec<_>>()
    );
}

/// An untyped value receiver stays UninferredReceiver — never a guess among
/// every method of that name.
#[test]
fn untyped_value_receiver_stays_uninferred_never_guesses() {
    let result = resolve_sources(&[
        (
            "a.rs",
            "pub struct A;\nimpl A { pub fn get(&self) -> u8 { 1 } }\n",
        ),
        (
            "b.rs",
            "pub struct B;\nimpl B { pub fn get(&self) -> u8 { 2 } }\n",
        ),
        (
            "c.rs",
            "pub struct C;\nimpl C { pub fn get(&self) -> u8 { 3 } }\n",
        ),
        (
            "use.rs",
            "pub fn read(map: &std::collections::HashMap<String, u8>) -> Option<&u8> {\n    map.get(\"k\")\n}\n",
        ),
    ]);

    let fanout: Vec<_> = result
        .edges
        .iter()
        .filter(|e| {
            e.edge_kind == EdgeKind::Calls
                && e.source_symbol.contains("read")
                && matches!(
                    e.resolution.as_deref(),
                    Some(Resolution::AmbiguousGlobal { .. })
                )
        })
        .collect();
    assert!(
        fanout.is_empty(),
        "map.get must not AmbiguousGlobal to A::get/B::get/C::get; got {:?}",
        fanout
            .iter()
            .map(|e| e.target_symbol.as_str())
            .collect::<Vec<_>>()
    );

    let entry = result
        .unresolved
        .iter()
        .find(|u| u.callee_name == "get" && u.source_symbol.contains("read"))
        .unwrap_or_else(|| panic!("map.get must be recorded: {:?}", result.unresolved));
    assert!(
        matches!(
            entry.class,
            UnresolvedClass::UninferredReceiver | UnresolvedClass::External { .. }
        ),
        "expected UninferredReceiver or External, got {:?}",
        entry.class
    );
}

/// Two types share a method name; an untyped receiver must not pick either.
#[test]
fn shared_method_name_on_untyped_receiver_never_guesses() {
    let result = resolve_sources(&[
        (
            "left.py",
            "class Left:\n    def run(self):\n        return 1\n",
        ),
        (
            "right.py",
            "class Right:\n    def run(self):\n        return 2\n",
        ),
        ("caller.py", "def go(obj):\n    return obj.run()\n"),
    ]);
    let edges: Vec<_> = result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol.contains("go"))
        .collect();
    assert!(
        edges.is_empty(),
        "obj.run() with no type must not bind Left or Right: {:?}",
        edges
            .iter()
            .map(|e| e.target_symbol.as_str())
            .collect::<Vec<_>>()
    );
}

/// Bare-name AmbiguousGlobal above the emission ceiling emits no edges —
/// one ledger row carries `candidate_total` instead of N capped edges.
#[test]
fn bare_name_above_fanout_ceiling_emits_ledger_not_edges() {
    let mut files: Vec<(String, String)> = (0..(AMBIGUOUS_FANOUT_CAP + 8))
        .map(|n| {
            (
                format!("d{n}.py"),
                "def spread():\n    return 1\n".to_string(),
            )
        })
        .collect();
    files.push((
        "caller.py".to_string(),
        "def go():\n    return spread()\n".to_string(),
    ));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let result = resolve_sources(&borrowed);

    let fanout: Vec<_> = result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol.contains("go"))
        .collect();
    assert!(
        fanout.is_empty(),
        "above the ceiling a bare ambiguous call must emit zero edges (got {}); \
         the ledger carries candidate_total instead of a capped sample",
        fanout.len()
    );

    let entry = result
        .unresolved
        .iter()
        .find(|u| u.callee_name == "spread" && u.source_symbol.contains("go"))
        .expect("the site must still be in the ledger");
    match &entry.resolution {
        Resolution::AmbiguousGlobal { candidates, .. } => {
            assert!(
                candidates.len() > AMBIGUOUS_FANOUT_CAP,
                "ledger must carry the full candidate list, got {}",
                candidates.len()
            );
        }
        other => {
            panic!("above-ceiling site must keep AmbiguousGlobal on the ledger row, got {other:?}")
        }
    }
}

/// Positive control: a typed local receiver still binds ReceiverType.
#[test]
fn typed_local_receiver_still_binds() {
    let result = resolve_fixture_dir("rust_typed_receiver");
    let tick_edges: Vec<_> = result
        .edges
        .iter()
        .filter(|e| {
            e.edge_kind == EdgeKind::Calls
                && e.target_symbol.contains("tick")
                && matches!(
                    e.resolution.as_deref(),
                    Some(Resolution::ReceiverType { .. } | Resolution::SameFile { .. })
                )
        })
        .collect();
    assert!(
        !tick_edges.is_empty(),
        "engine.tick() via typed local/param must still resolve; edges={:?} unresolved={:?}",
        result
            .edges
            .iter()
            .filter(|e| e.edge_kind == EdgeKind::Calls)
            .map(|e| format!("{}->{}", e.source_symbol, e.target_symbol))
            .collect::<Vec<_>>(),
        result
            .unresolved
            .iter()
            .map(|u| format!("{}:{:?}", u.callee_name, u.class))
            .collect::<Vec<_>>()
    );

    let via_local = tick_edges
        .iter()
        .any(|e| e.source_symbol.contains("via_local"));
    let via_field = tick_edges
        .iter()
        .any(|e| e.source_symbol.contains("via_field"));
    assert!(
        via_local,
        "Engine::new initializer must type the local for engine.tick(): {:?}",
        tick_edges
            .iter()
            .map(|e| e.source_symbol.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        via_field,
        "field hop h.engine.tick() must resolve through Holder.engine: {:?}",
        tick_edges
            .iter()
            .map(|e| e.source_symbol.as_str())
            .collect::<Vec<_>>()
    );
}

/// Go field-typed receiver: `w.Priority.valid()` one hop from the field Type ref.
#[test]
fn go_field_receiver_binds_through_field_type() {
    let result = resolve_fixture_dir("go_field_receiver");
    let edge = result.edges.iter().find(|e| {
        e.edge_kind == EdgeKind::Calls
            && e.target_symbol.contains("valid")
            && matches!(
                e.resolution.as_deref(),
                Some(Resolution::ReceiverType { .. })
            )
    });
    assert!(
        edge.is_some(),
        "w.Priority.valid() must resolve via field Type; edges={:?} unresolved={:?}",
        result
            .edges
            .iter()
            .filter(|e| e.edge_kind == EdgeKind::Calls)
            .map(|e| format!(
                "{}->{} {:?}",
                e.source_symbol, e.target_symbol, e.resolution
            ))
            .collect::<Vec<_>>(),
        result
            .unresolved
            .iter()
            .map(|u| format!("{}:{:?}:{:?}", u.callee_name, u.receiver, u.class))
            .collect::<Vec<_>>()
    );
}

/// Svelte runes are HostGlobal with environment `svelte`.
#[test]
fn svelte_runes_classify_as_host_global() {
    let result = resolve_fixture_dir("svelte_runes");
    for rune in ["$state", "$derived"] {
        let row = result
            .unresolved
            .iter()
            .find(|u| u.callee_name == rune)
            .unwrap_or_else(|| {
                panic!(
                    "{rune} must be in the ledger: {:?}",
                    result
                        .unresolved
                        .iter()
                        .map(|u| (&u.callee_name, &u.class))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            row.class,
            UnresolvedClass::HostGlobal {
                environment: "svelte".to_string()
            },
            "{rune} must be HostGlobal(svelte), got {:?}",
            row.class
        );
    }
}

/// A bare miss with no corpus namesake is NoNamesake, not Unresolved.
#[test]
fn bare_miss_with_no_namesake_is_no_namesake() {
    let result = resolve_sources(&[("a.py", "def go():\n    return totally_absent_helper()\n")]);
    let row = result
        .unresolved
        .iter()
        .find(|u| u.callee_name == "totally_absent_helper")
        .expect("must record the miss");
    assert_eq!(row.class, UnresolvedClass::NoNamesake);
}
