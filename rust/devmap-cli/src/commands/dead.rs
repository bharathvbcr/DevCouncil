use devmap_query::{ResolutionAvailability, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_json, emit_truncation, emit_unavailable, open_for_read};

#[derive(clap::Args)]
#[group(id = "Dead")]
pub(crate) struct Args {
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { budget } = args;
    let store = open_for_read(cli)?;
    let payload = StoreQueryEngine::new(&store).dead_symbols(*budget)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&payload)?)?;
    } else {
        emit_dead(&payload);
    }
    Ok(())
}

fn emit_dead(resp: &devmap_query::Response<devmap_analyze::DeadSymbolReport>) {
    if let ResolutionAvailability::Unavailable { reason } = &resp.resolution {
        emit_unavailable(reason);
        return;
    }
    // Two tiers, named — a single undifferentiated list is how a 0.4
    // `only_ambiguous_callers` row used to read as "safe to delete".
    let (confident, unconfirmed): (Vec<_>, Vec<_>) = resp
        .items
        .iter()
        .partition(|row| row.confidence > devmap_analyze::HIGHEST_DEGRADED_CONFIDENCE);
    if !confident.is_empty() {
        outln!("confident (no inbound evidence):");
        for row in &confident {
            emit_dead_row(row);
        }
    }
    if !unconfirmed.is_empty() {
        outln!("unconfirmed (ambiguous callers, namesake evidence, or coverage-capped):");
        for row in &unconfirmed {
            emit_dead_row(row);
        }
    }
    if resp.items.is_empty() {
        outln!("(no dead-symbol candidates in budget)");
    }
    emit_truncation(resp.shown, resp.hidden, resp.total, resp.truncated);
    if let Some(reason) = resp.walk_incomplete.as_ref() {
        outln!("\nwalk incomplete: {reason}");
    }
    emit_dead_clusters(resp);
}

fn emit_dead_row(row: &devmap_analyze::DeadSymbolReport) {
    match row.exemption_reason.as_deref() {
        Some(reason) if !reason.is_empty() => outln!(
            "{:.2}  {}::{}  ({reason})",
            row.confidence,
            row.file_path,
            row.symbol_name
        ),
        _ => outln!(
            "{:.2}  {}::{}",
            row.confidence,
            row.file_path,
            row.symbol_name
        ),
    }
}

/// The abandoned cycles, printed beside the single-symbol list rather than
/// inside it.
///
/// One line per cluster with a member sample: a forty-symbol dead subsystem is
/// *one* thing a reader acts on, and forty rows would push real single-symbol
/// findings past the budget. That is the same argument `manifest.rs` already
/// makes for the artifact.
///
/// `None` and an empty list are printed differently on purpose. "No abandoned
/// cycles" is a finding; "this generation predates the pass" is not, and a
/// reader deciding whether to rebuild needs to know which they are looking at.
fn emit_dead_clusters(resp: &devmap_query::Response<devmap_analyze::DeadSymbolReport>) {
    for line in dead_cluster_lines(resp) {
        outln!("{line}");
    }
}

/// The lines [`emit_dead_clusters`] prints, built rather than written straight
/// to stdout.
///
/// Split out so the branch order is assertable. The three outcomes read almost
/// alike to a human — "not computed", "not recorded", "none" — and the two that
/// differ by a single word give opposite advice, so which branch wins is the
/// property worth a test rather than a comment.
fn dead_cluster_lines(
    resp: &devmap_query::Response<devmap_analyze::DeadSymbolReport>,
) -> Vec<String> {
    // Order matters: a refusal is also an absence, and the rebuild advice below
    // is wrong for it — the next build walks the same oversized graph and
    // refuses again.
    if let Some(reason) = resp.dead_clusters_incomplete.as_ref() {
        return vec![format!("\nabandoned cycles: not computed — {reason}")];
    }
    let Some(clusters) = resp.dead_clusters.as_ref() else {
        return vec![
            "\nabandoned cycles: not recorded for this generation (rebuild with \
             `devmap build` to compute them)"
                .to_string(),
        ];
    };
    if clusters.is_empty() {
        return vec!["\nabandoned cycles: none".to_string()];
    }
    let mut lines = vec![format!(
        "\nabandoned cycles: {} component(s) nothing outside reaches",
        clusters.len()
    )];
    for cluster in clusters {
        let sample: Vec<&str> = cluster
            .members
            .iter()
            .take(DEAD_CLUSTER_SAMPLE)
            .map(String::as_str)
            .collect();
        let more = cluster.size.saturating_sub(sample.len());
        let tail = if more > 0 {
            format!(", +{more} more")
        } else {
            String::new()
        };
        lines.push(format!(
            "  {:.2}  {} symbols: {}{}",
            cluster.confidence,
            cluster.size,
            sample.join(", "),
            tail
        ));
    }
    if resp.dead_clusters_truncated > 0 {
        lines.push(format!(
            "  … {} further component(s) were found and not listed",
            resp.dead_clusters_truncated
        ));
    }
    lines
}

/// How many members of a cluster the human readout names before saying how many
/// more there are.
///
/// The producer already caps the stored list at `DEAD_CLUSTER_MEMBER_CAP`; this
/// is the *display* cap, and it is smaller because a terminal line is not a
/// place to read twenty-five qualified names. `cluster.size` is the real count
/// and is printed beside the sample, so a capped sample is never presented as a
/// whole membership.
const DEAD_CLUSTER_SAMPLE: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    /// A cluster scan that refused must never print as a scan that found none,
    /// and must never print the rebuild advice.
    ///
    /// The scan has three outcomes and `Option<Vec<_>>` holds two: a graph past
    /// `DEAD_CLUSTER_MAX_NODES` comes back with an *empty* cluster list beside
    /// a refusal flag. Mapping it field-for-field renders "too large to walk"
    /// as "no abandoned subsystems", and the human line under it tells the
    /// reader to rebuild — which walks the same graph and refuses again.
    ///
    /// All four states are asserted together because what is being pinned is
    /// the *branch order*, and a test of one branch cannot see an ordering.
    #[test]
    fn a_refused_cluster_scan_prints_neither_none_nor_rebuild_advice() {
        fn response(
            clusters: Option<Vec<devmap_analyze::DeadClusterReport>>,
            incomplete: Option<&str>,
        ) -> devmap_query::Response<devmap_analyze::DeadSymbolReport> {
            devmap_query::Response {
                source_freshness: devmap_query::SourceFreshness::unverified(
                    "whole-tree source freshness was not checked for this answer",
                ),
                items: Vec::new(),
                shown: 0,
                hidden: 0,
                total: 0,
                truncated: false,
                tokens_used: 0,
                resolution: devmap_query::ResolutionAvailability::Available,
                walk_incomplete: None,
                rungs: None,
                dead_clusters: clusters,
                dead_clusters_truncated: 0,
                dead_clusters_incomplete: incomplete.map(str::to_string),
                unresolved_namesakes: None,
                scope: None,
            }
        }
        let joined = |resp| dead_cluster_lines(&resp).join("\n");

        // 1. Refused: the reason, and neither of the other two readings.
        let refused = joined(response(
            None,
            Some("the call graph exceeded 400000 symbols"),
        ));
        assert!(
            refused.contains("not computed") && refused.contains("400000"),
            "a refusal must say so and say why: {refused:?}"
        );
        assert!(
            !refused.contains("none") && !refused.contains("rebuild"),
            "a refused scan is neither an empty finding nor a stale generation: {refused:?}"
        );

        // 2. Refused *and* holding a list — the shape the mapping is written to
        //    make impossible, asserted anyway, because the branch order is what
        //    keeps it impossible at the readout.
        let both = joined(response(Some(Vec::new()), Some("too large")));
        assert!(
            both.contains("not computed"),
            "the refusal wins over a list it also carries: {both:?}"
        );

        // 3. Absent: the generation predates the pass, and rebuilding does help.
        let absent = joined(response(None, None));
        assert!(
            absent.contains("not recorded") && absent.contains("rebuild"),
            "a generation with no scan is the one case rebuilding fixes: {absent:?}"
        );

        // 4. Ran and found none: a finding, and it must not read as either
        //    absence.
        let empty = joined(response(Some(Vec::new()), None));
        assert_eq!(
            empty.trim(),
            "abandoned cycles: none",
            "the pass ran; that is a result, not a caveat"
        );
    }
}
