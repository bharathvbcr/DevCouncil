use std::path::PathBuf;

use devmap_query::freshness;
use devmap_query::freshness::{FreshnessDigests, InventoryLimits, InventorySource};

use crate::cli::{default_root_hint, Cli, InventoryFlags};
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Freshness")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    /// The `generated_head` a map carries. Compared, never written.
    #[arg(long)]
    pub(crate) expect_head: Option<String>,
    /// The `indexed_hash` a map carries.
    #[arg(long)]
    pub(crate) expect_indexed_hash: Option<String>,
    /// The `content_fingerprint` a map carries.
    #[arg(long)]
    pub(crate) expect_content_fingerprint: Option<String>,
    /// Read the digest memo but never write it, for callers that must not
    /// modify the project — the MCP freshness probe runs on tools annotated
    /// `readOnlyHint: true`, and that annotation is a promise.
    #[arg(long)]
    pub(crate) no_cache_write: bool,
    #[command(flatten)]
    pub(crate) inventory: InventoryFlags,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        path,
        expect_head,
        expect_indexed_hash,
        expect_content_fingerprint,
        no_cache_write,
        inventory,
    } = args;
    let limits: InventoryLimits = (*inventory).into();
    let listing = freshness::inventory(path, limits);
    let mut digests = match &listing.source {
        InventorySource::Unavailable(reason) => FreshnessDigests {
            unavailable_reason: format!("git file inventory unavailable: {reason}"),
            ..Default::default()
        },
        InventorySource::Git => FreshnessDigests {
            generated_head: Some(freshness::git_head(path)).filter(|head| !head.is_empty()),
            indexed_hash: Some(freshness::files_fingerprint(&listing.files)),
            // Left for the short-circuit below to decide: hashing the
            // whole inventory is the expensive part, and a caller whose
            // head or file set has already moved has its answer.
            content_fingerprint: None,
            unavailable_reason: String::new(),
        },
    };

    // One field at a time, and only the fields the caller asked about.
    let compare = |expected: &Option<String>, actual: &Option<String>| {
        expected.as_deref().map(|expected| {
            let actual = actual.clone().unwrap_or_default();
            serde_json::json!({
                "stored": expected,
                "actual": actual,
                "match": expected == actual,
            })
        })
    };
    let head = compare(expect_head, &digests.generated_head);
    let files = compare(expect_indexed_hash, &digests.indexed_hash);
    let cheap_mismatch = [&head, &files]
        .into_iter()
        .flatten()
        .any(|checked| checked["match"] != serde_json::Value::Bool(true));
    // `RepoMapper.map_is_stale` computes the content fingerprint only
    // after head and inventory both match, and this has to cost what
    // that costs or delegating to it is a regression: hashing 1,385
    // files is ~110 ms against ~25 ms for the two `git ls-files` passes
    // that already answered. Computed anyway when nobody asked a
    // question, because then the digests *are* the answer.
    let content_wanted = !cheap_mismatch
        && (expect_content_fingerprint.is_some()
            || (expect_head.is_none() && expect_indexed_hash.is_none()));
    if content_wanted && listing.is_available() {
        digests.content_fingerprint = Some(freshness::content_fingerprint(
            path,
            &listing.files,
            !*no_cache_write,
        ));
    }
    let content = if content_wanted {
        compare(expect_content_fingerprint, &digests.content_fingerprint)
    } else {
        // Not "matched": *not checked*. A field whose check was skipped
        // must never report what a field that was checked and passed
        // reports, so it carries no `match` at all.
        expect_content_fingerprint.as_deref().map(|expected| {
            serde_json::json!({
                "stored": expected,
                "checked": false,
                "reason": "head or inventory already differ",
            })
        })
    };
    let asked = head.is_some() || files.is_some() || content.is_some();
    let mismatched: Vec<&str> = [
        ("head", &head),
        ("inventory", &files),
        ("content", &content),
    ]
    .into_iter()
    .filter_map(|(name, checked)| {
        checked
            .as_ref()
            .filter(|value| value.get("match") == Some(&serde_json::Value::Bool(false)))
            .map(|_| name)
    })
    .collect();

    // Fail closed. An inventory that could not be enumerated cannot
    // prove a map fresh, so the answer is "stale, and here is why it
    // could not be checked" — never "fresh" by absence of evidence.
    let (stale, reason) = if !digests.unavailable_reason.is_empty() {
        (asked.then_some(true), digests.unavailable_reason.clone())
    } else if !asked {
        (None, String::new())
    } else if mismatched.is_empty() {
        (Some(false), String::new())
    } else {
        (
            Some(true),
            format!(
                "{} changed since the map was written",
                mismatched.join(", ")
            ),
        )
    };

    emit_json(
        cli,
        &serde_json::json!({
            "path": path,
            "source": match listing.source {
                InventorySource::Git => "git",
                InventorySource::Unavailable(_) => "unavailable",
            },
            "unavailable_reason": digests.unavailable_reason,
            "generated_head": digests.generated_head,
            "indexed_hash": digests.indexed_hash,
            "content_fingerprint": digests.content_fingerprint,
            "files": listing.files.len(),
            // Class A: a capped inventory fingerprints a subset of the
            // tree, and the number it was cut from travels with it.
            "inventory_capped_from": listing.capped_from,
            "stale": stale,
            "reason": reason,
            "checked": {
                "head": head,
                "inventory": files,
                "content": content,
            },
        }),
    )?;
    Ok(())
}
