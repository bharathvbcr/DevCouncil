use std::path::{Path, PathBuf};

use devmap_query::freshness::InventoryLimits;
use devmap_query::{
    freshness, generate_code_graph_encodings, generate_manifest_with_edges,
    resolve_manifest_output, resolved_edge_from_stored, write_code_graph_atomically,
    write_manifest_atomically, ArtifactStamp, FreshnessInfo, StampedFreshness,
    CODE_GRAPH_SCHEMA_VERSION,
};
use devmap_store::Store;

use crate::cli::{default_root_hint, Cli, InventoryFlags, StampFlags};
use crate::output::{emit_json, ensure_parent, open_for_read};
use crate::{diagnostic, progress};

#[derive(clap::Args)]
#[group(id = "Manifest")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    /// Unset, resolved against `path`'s state directory. See
    /// `devmap_extract::paths`.
    #[arg(short, long)]
    pub(crate) output: Option<PathBuf>,
    /// Symbol-level graph companion artifact. Unset, resolved against
    /// `path`'s state directory.
    #[arg(long)]
    pub(crate) graph_output: Option<PathBuf>,
    /// Also write the interned encoding of the same graph here.
    ///
    /// Opt-in and additive: the verbose artifact above stays canonical and
    /// is written either way, because every existing consumer reads it.
    /// This form carries the identical model with each distinct string
    /// written once and referred to by index — on this repository
    /// 20,899,318 B becomes 4,951,872 B (-76.3%), `json.loads` 103.2 ms
    /// becomes 49.5 ms, write+fsync 10.7 ms becomes 4.2 ms. `source` and
    /// `target` alone were 52.6% of the verbose file: 14,324 distinct
    /// endpoint strings written 147,726 times.
    ///
    /// It does **not** make the graph readable by an agent — 5.2M tokens
    /// becomes 1.2M, which is still unopenable. It buys bytes, parse time
    /// and disk churn. Use `devmap search` / `impact` / `trace` to read the
    /// graph.
    #[arg(long)]
    pub(crate) compact_graph_output: Option<PathBuf>,
    /// Also write the marker-guarded agent guides, `AGENTS.md` and
    /// `CLAUDE.md`.
    ///
    /// Opt-in: creating two files in somebody's repository is not a thing a
    /// code index should do unasked. A guide that exists but carries no
    /// `Managed by devmap` marker is hand-written and is never touched.
    #[arg(long)]
    pub(crate) guides: bool,
    /// Replace a Python-schema or otherwise foreign repo map / code graph.
    #[arg(long, default_value_t = false)]
    pub(crate) force: bool,
    #[command(flatten)]
    pub(crate) stamps: StampFlags,
    #[command(flatten)]
    pub(crate) inventory: InventoryFlags,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        path,
        output,
        graph_output,
        compact_graph_output,
        guides,
        force,
        stamps,
        inventory,
    } = args;
    let store = open_for_read(cli)?;
    let outcome = write_consumer_artifacts(
        &store,
        ManifestRequest {
            progress: None,
            path,
            db: &cli.db(),
            output: &resolve_map_output(output, path),
            graph_output: &resolve_graph_output(graph_output, path),
            compact_graph_output: compact_graph_output.as_deref(),
            force: *force,
            stamps,
            inventory: (*inventory).into(),
            guides: *guides,
        },
    )?;
    report_manifest(cli, &outcome)?;
    Ok(())
}

/// Everything one `manifest` write needs, whether it was asked for on its own
/// or fused onto the end of a build.
pub(crate) struct ManifestRequest<'a> {
    pub(crate) progress: Option<&'a progress::Display>,
    /// The tree the caller named. Used only when the store cannot say where its
    /// repository root is.
    pub(crate) path: &'a std::path::Path,
    pub(crate) db: &'a std::path::Path,
    pub(crate) output: &'a std::path::Path,
    pub(crate) graph_output: &'a std::path::Path,
    pub(crate) compact_graph_output: Option<&'a std::path::Path>,
    pub(crate) force: bool,
    pub(crate) stamps: &'a StampFlags,
    pub(crate) inventory: InventoryLimits,
    /// Write the marker-guarded agent guides from this generation.
    pub(crate) guides: bool,
}

/// What a `manifest` write did, for the caller's `--json` payload.
pub(crate) struct ManifestOutcome {
    /// One entry per guide file considered, or empty when guides were not
    /// requested. Reported in full — including the files left alone and why —
    /// because "the guide was not refreshed" and "the guide is hand-written and
    /// therefore ours to leave" are different facts and only one is a problem.
    guides: Vec<devmap_query::guides::GuideOutcome>,
    output: std::path::PathBuf,
    graph_output: std::path::PathBuf,
    compact_graph_output: Option<std::path::PathBuf>,
    generation_id: u32,
    /// True when the artifacts on disk were already exactly the ones this run
    /// would have written, and the store was therefore never read.
    artifacts_unchanged: bool,
    /// `caller` / `kernel` / `unavailable` — where the three stamps came from.
    freshness_source: &'static str,
    freshness_unavailable_reason: String,
}

/// `<db>.artifacts.json` — the stamp beside the store the artifacts came from.
///
/// Beside the store rather than beside the artifacts: it describes what *this*
/// store's current generation produced, and two stores pointed at one output
/// path must not share one stamp. It also keeps it out of the git inventory,
/// so it can never change the fingerprint it helps compute.
fn artifact_stamp_path(db: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.artifacts.json", db.display()))
}

/// Write `repo_map.json` and `code_graph.json` from the store's current
/// generation — or prove they are already written and touch nothing.
///
/// The proof is a sidecar naming the binary, every input the artifacts derive
/// from, and each output's `(len, mtime, inode)` as written. When it holds, the
/// generation is never read out of SQLite and nothing is serialized: the case a
/// watcher and the PostToolUse hook hit on almost every tick.
/// Write `AGENTS.md` / `CLAUDE.md` from this generation, when asked.
///
/// Returns an empty vector when guides were not requested, which is the one
/// case that costs nothing: the manifest generation this needs is skipped
/// entirely rather than computed and discarded.
///
/// A failure here is fatal rather than a warning. The guide is what points an
/// agent at the map; a run that silently failed to refresh it leaves the tree
/// with a guide describing a generation that no longer exists, and nothing said
/// so.
fn write_guides_if_requested(
    store: &Store,
    request: &ManifestRequest<'_>,
    tree: &std::path::Path,
) -> anyhow::Result<Vec<devmap_query::guides::GuideOutcome>> {
    if !request.guides {
        return Ok(Vec::new());
    }
    let extractions = store.latest_extractions()?;
    let analysis = store
        .latest_analysis()?
        .ok_or_else(|| anyhow::anyhow!("guides unavailable: build a persisted generation first"))?;
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    // Stamps the guide never reads. Passing the real ones would mean computing
    // the digests first, which is the ordering this whole function exists to
    // avoid; passing placeholders is safe precisely because `guides.rs` reads
    // `subsystems`, `important_files` and `meta.devmap_rust` and nothing else.
    let placeholder = FreshnessInfo {
        head_sha: String::new(),
        generation_id: 0,
        pending_count: 0,
        stamped: StampedFreshness::default(),
    };
    let (_manifest, json_str) =
        generate_manifest_with_edges(&extractions, &analysis, placeholder, &edges, Some(tree));
    let map: serde_json::Value = serde_json::from_str(&json_str)?;

    let relative = |absolute: &std::path::Path| -> String {
        let text = absolute
            .strip_prefix(tree)
            .unwrap_or(absolute)
            .to_string_lossy()
            .replace('\\', "/");
        // `--db` defaults to a CWD-relative path, so stripping the tree prefix
        // can leave `./.devmap/...`. The guide is prose an agent reads and
        // copies; a stray `./` is noise in every line that quotes a path.
        text.strip_prefix("./").unwrap_or(&text).to_string()
    };
    Ok(devmap_query::guides::write_agent_guides(
        tree,
        &map,
        &relative(&devmap_extract::paths::repo_map_path(tree)),
        &relative(&devmap_extract::paths::code_graph_path(tree)),
        &relative(request.db),
    )?)
}

pub(crate) fn write_consumer_artifacts(
    store: &Store,
    request: ManifestRequest<'_>,
) -> anyhow::Result<ManifestOutcome> {
    let gen_id = store.latest_generation_id()?.ok_or_else(|| {
        anyhow::anyhow!("manifest unavailable: build a persisted generation first")
    })?;
    let status = store.status(&request.db.display().to_string())?;
    let built_head = store
        .latest_generation_head()?
        .unwrap_or_else(|| "unavailable".to_string());
    let repo_root = store.latest_repo_root()?.or_else(|| {
        request
            .path
            .canonicalize()
            .ok()
            .map(|root| root.to_string_lossy().into_owned())
    });

    // The tree the digests describe is the one the store indexed. Falling back
    // to the caller's path only when the store cannot say keeps the stamps and
    // the generation talking about the same directory.
    let tree = repo_root
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| request.path.to_path_buf());

    // The guides go in **before** the digests are taken, not after.
    //
    // A guide this run creates or rewrites is a file in the tree, and unless the
    // repository ignores it, it is part of the inventory `freshness::compute`
    // hashes. Written afterwards it would move `content_fingerprint` the instant
    // it landed, and the map would report itself stale against a tree only it
    // had changed — a false staleness that costs a rebuild on every single run.
    //
    // The guide's text depends on the manifest's subsystems and provenance
    // markers but not on its freshness stamps, so generating the manifest early
    // to feed the guide and again afterwards with real stamps is well-founded:
    // the second generation cannot change what the first one said here. The
    // early generation is skipped entirely unless guides were asked for.
    let guides = write_guides_if_requested(store, &request, &tree)?;

    let (stamped, freshness_source, freshness_unavailable_reason) = match request.stamps.supplied()
    {
        Some(supplied) => (supplied, "caller", String::new()),
        None => {
            let digests = freshness::compute(&tree, request.inventory, true);
            let source = if digests.unavailable_reason.is_empty() {
                "kernel"
            } else {
                "unavailable"
            };
            (
                StampedFreshness {
                    generated_head: digests.generated_head,
                    indexed_hash: digests.indexed_hash,
                    content_fingerprint: digests.content_fingerprint,
                },
                source,
                digests.unavailable_reason,
            )
        }
    };

    let dest = resolve_manifest_output(repo_root.as_deref(), request.output);
    let graph_dest = resolve_manifest_output(repo_root.as_deref(), request.graph_output);
    let compact_dest = request
        .compact_graph_output
        .map(|destination| resolve_manifest_output(repo_root.as_deref(), destination));

    // Every input the artifacts' bytes derive from, as real JSON. These values
    // are compared for equality to decide a skip, and they are also the only
    // record of *why* a given set of artifacts exists, so a consumer has to be
    // able to read them. `serde_json::Value` keeps the property the previous
    // `{:?}` renderings were reaching for — `null` and `""` are different
    // values, so a digest that could not be computed can never compare equal to
    // one that came out empty — without the file being JSON in syntax only.
    let mut inputs: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    inputs.insert("generation_id".into(), gen_id.into());
    inputs.insert("pending_count".into(), status.pending_count.into());
    inputs.insert("built_head".into(), built_head.clone().into());
    inputs.insert("repo_root".into(), repo_root.clone().into());
    inputs.insert(
        "generated_head".into(),
        stamped.generated_head.clone().into(),
    );
    inputs.insert("indexed_hash".into(), stamped.indexed_hash.clone().into());
    inputs.insert(
        "content_fingerprint".into(),
        stamped.content_fingerprint.clone().into(),
    );
    inputs.insert("code_graph_schema".into(), CODE_GRAPH_SCHEMA_VERSION.into());
    // The one input that moves with the clock rather than the tree: churn is
    // `git log --since=90.days`, relative to now, so the same repository on a
    // later day is a different window. Without this the artifacts of a quiet
    // repository matched every input for months while their hotspot counts
    // silently shrank. Day granularity: one regeneration per calendar day at
    // most, and only on a run that would otherwise have skipped.
    inputs.insert(
        "churn_window_day".into(),
        devmap_query::inventory::churn_window_day().into(),
    );
    inputs.insert(
        "compact".into(),
        match &compact_dest {
            Some(path) => path.to_string_lossy().into_owned().into(),
            None => serde_json::Value::Null,
        },
    );

    // Taken before `stamped` is consumed below; the stamp is written at the end
    // of the run, long after it has been moved into the manifest.
    let stamp_generated_head = stamped.generated_head.clone();

    let stamp_path = artifact_stamp_path(request.db);
    // Role, not position: the sidecar is read by consumers that cannot rebuild
    // the writer's spelling of these paths, so each output is named.
    let mut outputs: Vec<(&str, &std::path::Path)> = vec![
        ("repo_map", dest.as_path()),
        ("code_graph", graph_dest.as_path()),
    ];
    if let Some(compact) = &compact_dest {
        outputs.push(("compact_graph", compact.as_path()));
    }
    if ArtifactStamp::read(&stamp_path).is_some_and(|stamp| stamp.still_current(&inputs, &outputs))
    {
        return Ok(ManifestOutcome {
            output: dest,
            graph_output: graph_dest,
            compact_graph_output: compact_dest,
            generation_id: gen_id,
            artifacts_unchanged: true,
            guides,
            freshness_source,
            freshness_unavailable_reason,
        });
    }

    let extractions = store.latest_extractions()?;
    let analysis = store.latest_analysis()?.ok_or_else(|| {
        anyhow::anyhow!("manifest unavailable: build a persisted generation first")
    })?;
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    // One freshness identity for both artifacts: a map and a graph stamped from
    // different generations is the drift the single command exists to prevent.
    let freshness = FreshnessInfo {
        head_sha: built_head,
        generation_id: gen_id,
        pending_count: status.pending_count,
        stamped,
    };
    let (_manifest, json_str) = generate_manifest_with_edges(
        &extractions,
        &analysis,
        freshness.clone(),
        &edges,
        Some(tree.as_path()),
    );
    let (graph_json, compact_graph_json) = generate_code_graph_encodings(
        &extractions,
        &analysis,
        &edges,
        &freshness,
        repo_root.as_deref(),
        compact_dest.is_some(),
    )?;

    ensure_parent(&dest)?;
    write_manifest_atomically(&dest, &json_str, request.force)?;
    ensure_parent(&graph_dest)?;
    write_code_graph_atomically(&graph_dest, &graph_json, request.force)?;
    // Written through the same clobber guard as the verbose artifact. A foreign
    // file at this path is refused for the same reason: the guard's question is
    // "did this kernel write what is already here", and the answer does not
    // depend on the encoding.
    if let (Some(destination), Some(json)) = (&compact_dest, &compact_graph_json) {
        ensure_parent(destination)?;
        write_code_graph_atomically(destination, json, request.force)?;
    }

    // The stamp last, and only after every write succeeded: a stamp claiming
    // artifacts that were never written is a skip that skips nothing real.
    // A stamp that cannot be written is not fatal — it costs the next run a
    // regeneration, which is the behaviour that existed before the stamp.
    let note = |message: String| {
        if let Some(progress) = request.progress {
            progress.diagnostic(message);
        } else {
            diagnostic(format_args!("{message}"));
        }
    };
    match ArtifactStamp::of(inputs, stamp_generated_head, &outputs) {
        Ok(stamp) => {
            if let Err(error) = stamp.write(&stamp_path) {
                note(format!(
                    "  note: could not record the artifact stamp at {} ({error}); \
                     the next manifest will regenerate rather than skip",
                    stamp_path.display()
                ));
            }
        }
        Err(error) => note(format!(
            "  note: could not stat the artifacts just written ({error}); \
             the next manifest will regenerate rather than skip"
        )),
    }

    Ok(ManifestOutcome {
        output: dest,
        graph_output: graph_dest,
        compact_graph_output: compact_dest,
        generation_id: gen_id,
        artifacts_unchanged: false,
        guides,
        freshness_source,
        freshness_unavailable_reason,
    })
}

/// The `manifest` result, as JSON or as the two human lines it always printed.
pub(crate) fn report_manifest(cli: &Cli, outcome: &ManifestOutcome) -> anyhow::Result<()> {
    if cli.json {
        return emit_json(cli, &manifest_json(outcome));
    }
    if outcome.artifacts_unchanged {
        outln!(
            "Artifacts already current for generation #{} ({:?}, {:?}).",
            outcome.generation_id,
            outcome.output,
            outcome.graph_output
        );
    } else {
        outln!("Manifest written to {:?}", outcome.output);
        outln!("Code graph written to {:?}", outcome.graph_output);
        if let Some(destination) = &outcome.compact_graph_output {
            outln!("Interned code graph written to {:?}", destination);
        }
    }
    for guide in &outcome.guides {
        let note = match guide.disposition {
            devmap_query::guides::GuideDisposition::Created => "created",
            devmap_query::guides::GuideDisposition::Updated => "updated",
            devmap_query::guides::GuideDisposition::Unchanged => "already current",
            devmap_query::guides::GuideDisposition::NotOurs => {
                "left alone (no `Managed by devmap` marker)"
            }
        };
        outln!("  guide {}: {note}", guide.path.display());
    }
    if !outcome.freshness_unavailable_reason.is_empty() {
        outln!(
            "  freshness stamps unavailable: {}",
            outcome.freshness_unavailable_reason
        );
    }
    Ok(())
}

pub(crate) fn manifest_json(outcome: &ManifestOutcome) -> serde_json::Value {
    serde_json::json!({
        "output": outcome.output,
        "graph_output": outcome.graph_output,
        // Absent, not empty, when no interned artifact was asked for: `""`
        // would read as a path that failed.
        "compact_graph_output": outcome.compact_graph_output,
        "generation_id": outcome.generation_id,
        // The artifacts on disk were already the ones this run would write, so
        // the generation was never read and nothing was serialized. Reported
        // rather than left silent: a caller timing this command needs to know
        // which of the two paths it measured.
        "artifacts_unchanged": outcome.artifacts_unchanged,
        "freshness_source": outcome.freshness_source,
        "freshness_unavailable_reason": outcome.freshness_unavailable_reason,
        // Every file considered, with what happened to it. A guide left alone
        // because it is hand-written is reported as such rather than omitted:
        // "not refreshed" and "not ours to refresh" are different facts, and a
        // caller that cannot tell them apart cannot tell a working install from
        // a guide that silently stopped tracking the map.
        "guides": outcome.guides.iter().map(|guide| serde_json::json!({
            "path": guide.path,
            "disposition": match guide.disposition {
                devmap_query::guides::GuideDisposition::Created => "created",
                devmap_query::guides::GuideDisposition::Updated => "updated",
                devmap_query::guides::GuideDisposition::Unchanged => "unchanged",
                devmap_query::guides::GuideDisposition::NotOurs => "not_ours",
            },
            "changed": guide.changed(),
        })).collect::<Vec<_>>(),
    })
}

/// Where `repo_map.json` goes for this invocation.
///
/// An explicit `--output` is used as given. Otherwise the artifact lands in
/// whichever state directory `root` resolves to, so the map is written beside
/// the store that produced it rather than into a directory chosen by the
/// caller's shell.
pub(crate) fn resolve_map_output(explicit: &Option<PathBuf>, root: &Path) -> PathBuf {
    explicit
        .clone()
        .unwrap_or_else(|| devmap_extract::paths::repo_map_path(root))
}

/// Where `code_graph.json` goes for this invocation. See [`resolve_map_output`].
pub(crate) fn resolve_graph_output(explicit: &Option<PathBuf>, root: &Path) -> PathBuf {
    explicit
        .clone()
        .unwrap_or_else(|| devmap_extract::paths::code_graph_path(root))
}
