use std::path::PathBuf;

use crate::cli::{default_root_hint, Cli};

#[derive(clap::Args)]
#[group(id = "MapHtml")]
pub(crate) struct Args {
    /// Repository root; `--input` and `--output` resolve against it.
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    /// The repo map to render. Defaults to the resolved state directory.
    #[arg(long)]
    pub(crate) input: Option<PathBuf>,
    /// Where to write the page. Defaults to `<state dir>/map.html`.
    #[arg(short, long)]
    pub(crate) output: Option<PathBuf>,
    /// Rewrite even when the existing page already carries this map's
    /// fingerprint.
    #[arg(long, default_value_t = false)]
    pub(crate) force: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        path,
        input,
        output,
        force,
    } = args;
    let resolve = |p: &PathBuf| -> PathBuf {
        if p.is_absolute() {
            p.clone()
        } else {
            path.join(p)
        }
    };
    let map_path = input
        .as_ref()
        .map(resolve)
        .unwrap_or_else(|| devmap_extract::paths::repo_map_path(path));
    let out_path = output
        .as_ref()
        .map(resolve)
        .unwrap_or_else(|| devmap_extract::paths::state_dir(path).join("map.html"));

    let repo_map =
        devmap_query::host::read_repo_map(&map_path, devmap_query::host::DEFAULT_ARTIFACT_BYTES)
            .map_err(|err| {
                anyhow::anyhow!("cannot load repo map at {}: {err}", map_path.display())
            })?;
    let fingerprint = devmap_query::fingerprint_for(&repo_map);

    // Skip an unchanged rewrite so a watch tick does not churn the file
    // — but only when the map carries stamps to fingerprint. An unstamped
    // map fingerprints to a constant, and skipping on that would pin the
    // page to whatever was rendered first.
    let stamped = !fingerprint.generated_head.is_empty();
    let regenerate = *force || !stamped || devmap_query::should_regenerate(&out_path, &fingerprint);
    if !regenerate {
        if cli.json {
            outln!(
                "{}",
                serde_json::json!({
                    "output": out_path.display().to_string(),
                    "written": false,
                    "reason": "unchanged",
                    "fingerprint": fingerprint.fingerprint,
                })
            );
        } else {
            outln!("{} is current", out_path.display());
        }
        return Ok(());
    }

    let html = devmap_query::render_map_preview_html(&repo_map, &fingerprint);
    devmap_query::write_atomic(&out_path, html.as_bytes())?;
    if cli.json {
        outln!(
            "{}",
            serde_json::json!({
                "output": out_path.display().to_string(),
                "written": true,
                "bytes": html.len(),
                "fingerprint": fingerprint.fingerprint,
            })
        );
    } else {
        outln!("Wrote {} ({} bytes)", out_path.display(), html.len());
    }
    Ok(())
}
