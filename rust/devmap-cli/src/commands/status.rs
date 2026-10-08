use devmap_store::Store;

use crate::cli::Cli;
use crate::output::emit_json;
use crate::{on_command_stack, write_stdout_raw};

#[derive(clap::Args)]
#[group(id = "Status")]
pub(crate) struct Args {
    /// When `rebuild_required`, run a bounded `devmap build` before answering.
    ///
    /// Distinct from `is_fresh`: source drift and pending edits stay
    /// report-only; only payload-obsolete and schema-behind auto-rebuild.
    #[arg(long)]
    pub(crate) auto_rebuild: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { auto_rebuild } = args;
    // Answers even with no store, but never creates one. The client
    // treats a missing store as "not built yet"; creating it here made
    // that a race (see `Store::open_existing`).
    //
    // K3: the schema is probed *before* the store is opened, because
    // `Store::open` runs the migration chain under an exclusive
    // transaction from every open. This command used to rewrite the
    // schema of a store it was only asked to describe, silently, on the
    // one command a health check runs against a store it does not own.
    // Migrating is `build`'s job, where the caller asked for a write.
    let stored_schema = Store::stored_schema_version(cli.db())?;
    let Some(stored_schema) = stored_schema else {
        let mut payload = serde_json::json!({
            "generation_id": serde_json::Value::Null,
            "pending_count": 0,
            "node_count": 0,
            "edge_count": 0,
            "is_fresh": false,
            "db_path": cli.db().display().to_string(),
            "degraded_reason": "no devmap store at this path (run `devmap build`)",
            "quarantined_count": 0,
            "quarantined_paths": Vec::<String>::new(),
            // `null`, never three empty lists. Nothing was measured
            // here — there is no store to measure — and an empty
            // inventory is the answer of a build that read everything.
            "coverage_gaps": serde_json::Value::Null,
            "edge_resolution_source": serde_json::Value::Null,
            "edge_confidence_mismatches": serde_json::Value::Null,
            // Nothing was measured — same rule as coverage_gaps.
            "resolution_rate": serde_json::Value::Null,
            "schema_outdated": false,
            "schema_version": serde_json::Value::Null,
            "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
            // A property of the binary, not of the store — so it is
            // answered even here, where there is no store. This is the
            // exit the seam's own probe takes.
            "capabilities": kernel_capabilities(),
        });
        if let Some(obj) = payload.as_object_mut() {
            attach_rebuild_fields(obj, false);
        }
        extend_host_contract(&mut payload, None, None);
        emit_json(cli, &payload)?;
        return Ok(());
    };
    if stored_schema != devmap_store::CURRENT_SCHEMA_VERSION {
        let version = stored_schema;
        let migratable = Store::schema_is_migratable(version);
        let mut payload = serde_json::json!({
            "generation_id": serde_json::Value::Null,
            "pending_count": 0,
            "node_count": 0,
            "edge_count": 0,
            "is_fresh": false,
            "db_path": cli.db().display().to_string(),
            // `user_version = 2` is the Python engine's `index.sqlite`, a
            // schema this kernel has no migration for. `devmap build`
            // against it already refuses by name (the store's own
            // message); telling `status` readers to run it is advice
            // that cannot work, for a file the other command names.
            "degraded_reason": if version == devmap_store::PYTHON_INDEX_SCHEMA_VERSION {
                format!(
                    "store schema is {version}: this is the Python engine's database \
                     (`.devcouncil/codeintel/index.sqlite`), not a devmap store, and \
                     this kernel cannot convert it — point `--db` at `devmap.sqlite` \
                     (this binary speaks {})",
                    devmap_store::CURRENT_SCHEMA_VERSION
                )
            } else if version > devmap_store::CURRENT_SCHEMA_VERSION {
                format!(
                    "store schema is {version}, newer than the {} this binary speaks; \
                     install a matching or newer devmap binary",
                    devmap_store::CURRENT_SCHEMA_VERSION
                )
            } else if migratable {
                format!(
                    "store schema is {version}, this binary speaks {}; \
                     run `devmap build` to migrate it",
                    devmap_store::CURRENT_SCHEMA_VERSION
                )
            } else {
                format!(
                    "store schema is {version}, unsupported by this binary (which speaks {})",
                    devmap_store::CURRENT_SCHEMA_VERSION
                )
            },
            "quarantined_count": 0,
            "quarantined_paths": Vec::<String>::new(),
            // Same reason as the no-store case: this binary refused to
            // read the store, so it measured nothing.
            "coverage_gaps": serde_json::Value::Null,
            "edge_resolution_source": serde_json::Value::Null,
            "edge_confidence_mismatches": serde_json::Value::Null,
            // Store refused — nothing measured, not a zero rate.
            "resolution_rate": serde_json::Value::Null,
            "schema_outdated": true,
            "schema_version": version,
            "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
            "capabilities": kernel_capabilities(),
        });
        if let Some(obj) = payload.as_object_mut() {
            // Only migratable schema-behind auto-rebuilds; a newer or
            // Python store cannot be fixed by `devmap build`.
            attach_rebuild_fields(obj, migratable);
        }
        if *auto_rebuild
            && payload
                .get("rebuild_required")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            match run_auto_rebuild(cli) {
                Ok(()) => {
                    // Re-enter status after a successful migrate.
                    // Avoid recursion through clap: re-open below by
                    // falling through is awkward; re-exec status.
                    let exe = std::env::current_exe()?;
                    let mut cmd = std::process::Command::new(exe);
                    cmd.arg("--json").arg("--db").arg(cli.db()).arg("status");
                    let out = cmd.output()?;
                    write_stdout_raw(format_args!("{}", String::from_utf8_lossy(&out.stdout)));
                    return Ok(());
                }
                Err(err) => {
                    if let Some(obj) = payload.as_object_mut() {
                        obj.insert("auto_rebuild_error".into(), serde_json::json!(err));
                    }
                }
            }
        }
        extend_host_contract(&mut payload, Some(version), None);
        emit_json(cli, &payload)?;
        return Ok(());
    }
    let Some(store) = Store::open_existing(cli.db())? else {
        anyhow::bail!(
            "the devmap store at {} vanished between the schema probe and the read",
            cli.db().display()
        );
    };
    if cli.db.is_none() {
        store.validate_repo_root(&cli.root_hint())?;
    }
    let mut payload = store_status_fields(&store, &cli.db())?;
    payload.insert("schema_outdated".into(), serde_json::json!(false));
    payload.insert("schema_version".into(), serde_json::json!(stored_schema));
    payload.insert(
        "expected_schema_version".into(),
        serde_json::json!(devmap_store::CURRENT_SCHEMA_VERSION),
    );
    payload.insert("capabilities".into(), kernel_capabilities());
    attach_rebuild_fields(&mut payload, false);
    if *auto_rebuild
        && payload
            .get("rebuild_required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        match run_auto_rebuild(cli) {
            Ok(()) => {
                // Refresh fields after rebuild.
                if let Some(store) = Store::open_existing(cli.db())? {
                    payload = store_status_fields(&store, &cli.db())?;
                    payload.insert("schema_outdated".into(), serde_json::json!(false));
                    payload.insert("schema_version".into(), serde_json::json!(stored_schema));
                    payload.insert(
                        "expected_schema_version".into(),
                        serde_json::json!(devmap_store::CURRENT_SCHEMA_VERSION),
                    );
                    payload.insert("capabilities".into(), kernel_capabilities());
                    attach_rebuild_fields(&mut payload, false);
                    payload.insert("auto_rebuilt".into(), serde_json::json!(true));
                }
            }
            Err(err) => {
                payload.insert("auto_rebuild_error".into(), serde_json::json!(err));
            }
        }
    }
    payload.extend(host_contract_fields(
        Some(stored_schema),
        payload
            .get("generation_id")
            .and_then(serde_json::Value::as_i64),
    ));
    emit_json(cli, &serde_json::Value::Object(payload))?;
    Ok(())
}

/// The store fields `devmap status` reports, as one object.
///
/// One owner, because two callers ask for them now: `status` itself, and the
/// build that writes the artifacts, which embeds them so the seam does not have
/// to spawn a third process to learn what the store it just wrote looks like.
/// The schema keys are *not* here — they come from a probe `status` runs before
/// it opens the store at all, and a build has already opened it.
///
/// What this kernel can be asked to do, read out of its own parser.
///
/// The seam used to learn this by running `devmap manifest --help` and
/// `devmap build --help` and grepping the output — two extra process launches
/// (~140 ms each, measured) per `dev map`, on top of the `status` probe it
/// already runs to rank candidate binaries. `status` is the probe that has to
/// happen anyway, so it is the one that should answer.
///
/// Derived from clap's command tree rather than asserted, because a hand-written
/// `true` is a claim that drifts the moment a flag is renamed: this cannot
/// declare a flag the binary does not actually accept. A kernel too old to carry
/// this key declares nothing, and the seam falls back to the `--help` probe —
/// "no evidence" must not read as "does not support it".
fn kernel_capabilities() -> serde_json::Value {
    on_command_stack(|| {
        use clap::CommandFactory;
        let command = Cli::command();
        let accepts = |subcommand: &str, flag: &str| -> bool {
            command
                .get_subcommands()
                .find(|candidate| candidate.get_name() == subcommand)
                .is_some_and(|candidate| {
                    candidate
                        .get_arguments()
                        .any(|argument| argument.get_long() == Some(flag))
                })
        };
        let has_command = |subcommand: &str| -> bool {
            command
                .get_subcommands()
                .any(|candidate| candidate.get_name() == subcommand)
        };
        // All three or none: a kernel accepting only some of the digests would need
        // the read-modify-write path for the rest, and running both is strictly
        // worse than running one.
        let stamp_flags = ["generated-head", "indexed-hash", "content-fingerprint"]
            .iter()
            .all(|flag| accepts("manifest", flag));
        serde_json::json!({
            "status": has_command("status"),
            "search": has_command("search"),
            "explore": has_command("explore"),
            "impact": has_command("impact"),
            "trace": has_command("trace"),
            "affected": has_command("affected"),
            "html": has_command("html"),
            "manifest_graph_output": accepts("manifest", "graph-output"),
            "manifest_stamp_flags": stamp_flags,
            "build_manifest": accepts("build", "manifest"),
        })
    })
}

/// Stable compatibility fields for process hosts such as Manvi and GitPulse.
///
/// `status` intentionally exits successfully for incompatible stores so a host
/// can inspect this contract before deciding whether to invoke a query. These
/// fields therefore carry readiness explicitly rather than making exit status
/// stand in for schema negotiation.
fn host_contract_fields(
    stored_schema: Option<i32>,
    generation_id: Option<i64>,
) -> serde_json::Map<String, serde_json::Value> {
    let relation = match stored_schema {
        None => "missing",
        Some(version) if version == devmap_store::CURRENT_SCHEMA_VERSION => "current",
        Some(version) if version == devmap_store::PYTHON_INDEX_SCHEMA_VERSION => "foreign",
        Some(version) if version > devmap_store::CURRENT_SCHEMA_VERSION => "newer",
        Some(version) if Store::schema_is_migratable(version) => "upgradeable",
        Some(_) => "unsupported",
    };
    let reader_ready = relation == "current";
    serde_json::Map::from_iter([
        ("host_contract_version".into(), serde_json::json!(1)),
        (
            "binary_version".into(),
            serde_json::json!(env!("CARGO_PKG_VERSION")),
        ),
        ("schema_relation".into(), serde_json::json!(relation)),
        ("reader_ready".into(), serde_json::json!(reader_ready)),
        (
            "query_ready".into(),
            serde_json::json!(reader_ready && generation_id.is_some()),
        ),
    ])
}

fn extend_host_contract(
    value: &mut serde_json::Value,
    stored_schema: Option<i32>,
    generation_id: Option<i64>,
) {
    if let Some(fields) = value.as_object_mut() {
        fields.extend(host_contract_fields(stored_schema, generation_id));
    }
}

pub(crate) fn store_status_fields(
    store: &Store,
    db: &std::path::Path,
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let status = store.status(&db.display().to_string())?;
    // K-A2: the graph's own degradation belongs in the answer a health check
    // reads.
    //
    // `freshness_degraded_reason` describes the *index* — no generation
    // persisted, paths stuck in the retry queue — and said nothing about a
    // generation built from a corpus the extractor could not read in full. That
    // is how a repository whose only caller of a symbol was refused for being
    // oversized reported `degraded_reason: null` while both artifacts of the
    // same build carried `graph_degraded: true`. Both degradations can hold at
    // once and neither may shadow the other, so they are joined with
    // `devmap_analyze::combine_reasons`, the same joiner the analysis uses for
    // its own pair.
    let analysis_degraded = match store.latest_analysis_status()? {
        Some(devmap_analyze::model::AnalysisStatus::Ok) | None => None,
        Some(devmap_analyze::model::AnalysisStatus::Partial { reason }) => {
            Some(format!("partial: {reason}"))
        }
        Some(devmap_analyze::model::AnalysisStatus::Timeout { reason }) => {
            Some(format!("timeout: {reason}"))
        }
    };
    let degraded_reason = devmap_analyze::combine_reasons(
        devmap_serve::freshness_degraded_reason(&status),
        analysis_degraded,
    );
    let serde_json::Value::Object(fields) = serde_json::json!({
        "generation_id": status.latest_generation,
        "pending_count": status.pending_count,
        "node_count": status.node_count,
        "edge_count": status.edge_count,
        // K-A6: one owner for this rule, shared with the daemon's `status`.
        // Computing it here as `pending_count == 0` is what let a store with no
        // generation at all report as current.
        "is_fresh": devmap_serve::index_is_fresh(&status),
        "source_freshness": status.source_freshness,
        "analyzer_freshness": status.analyzer_freshness,
        "db_path": status.db_path,
        "degraded_reason": degraded_reason,
        "delta": status.source_delta.as_ref().map(|delta| serde_json::json!({
            "added": delta.added,
            "changed": delta.changed,
            "removed": delta.removed,
            "sample_paths": delta.sample_paths,
        })),
        "quarantined_count": status.quarantined_count,
        // K1(g): naming the stuck paths is what makes a degraded status
        // actionable — "64 path(s) exceeded the retry threshold" told an
        // operator nothing about which 64.
        "quarantined_paths": status.quarantined_paths,
        // The same argument, one surface over. `degraded_reason` has always
        // carried "2 file(s) failed to parse, 1 recovered by pattern, 1 refused
        // by discovery" and never a single path, so an operator could not tell
        // a correct refusal — this repository's is a 30.6 MB vendored
        // `parser.c` against a 1 MiB ceiling — from a broken one without
        // opening the database. Rendered by `devmap_serve::coverage_gaps_json`,
        // shared with the daemon's own `status`.
        "coverage_gaps": devmap_serve::coverage_gaps_json(&status),
        // Whether this generation's edges carry the evidence the resolver
        // recorded, or a reconstruction standing in for one it never stored.
        // `null` when there is no generation or it holds no edges — which is
        // "nothing to say", not "reconstructed".
        "edge_resolution_source": store
            .latest_edge_resolution_source()?
            .map(|source| source.label()),
        // The read-side half of the honesty invariant: stored edges whose
        // confidence contradicts the resolution kind recorded for them. On the
        // way in `ResolvedEdge::resolved` makes the two agree; this reads them
        // back separately and counts the rows that no longer do. Counted in
        // SQL because this is a fresh process per call and must not build the
        // edge index for one number. `null` with no generation; 0 is a
        // measurement, never a default.
        "edge_confidence_mismatches": store.edge_confidence_mismatches()?,
        // Same object `devmap build` prints: persisted on the generation, not
        // recomputed. `null` when absent (no generation, or a summary written
        // before the field existed) — unexplained is not zero.
        "resolution_rate": store.latest_resolution_rate()?,
    }) else {
        unreachable!("json! of an object literal is an object")
    };
    Ok(fields)
}

/// `rebuild_required` / `rebuild_reason` — distinct from `is_fresh`.
///
/// Source drift and pending edits make an index stale without wanting an
/// automatic SessionStart rebuild; payload-obsolete and schema-behind do.
fn attach_rebuild_fields(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    schema_outdated: bool,
) {
    let degraded = fields
        .get("degraded_reason")
        .and_then(serde_json::Value::as_str);
    let reason = devmap_serve::rebuild_required_reason(degraded, schema_outdated);
    fields.insert(
        "rebuild_required".into(),
        serde_json::json!(reason.is_some()),
    );
    fields.insert(
        "rebuild_reason".into(),
        match reason {
            Some(r) => serde_json::json!(r),
            None => serde_json::Value::Null,
        },
    );
}

/// Run a child `devmap build` for SessionStart auto-rebuild. Stdout is
/// discarded so the parent's status JSON stays the only line on the wire;
/// stderr keeps progress. Failure is reported in the status payload rather
/// than aborting the hook — a session that cannot rebuild must still start.
fn run_auto_rebuild(cli: &Cli) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--db").arg(cli.db());
    cmd.arg("build").arg(cli.root_hint());
    cmd.stdout(std::process::Stdio::null());
    let output = cmd.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "auto-rebuild exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.chars().take(500).collect::<String>()
        ))
    }
}
