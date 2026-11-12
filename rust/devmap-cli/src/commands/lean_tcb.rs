use std::path::PathBuf;

use anyhow::bail;
use devmap_query::lean_tcb::{
    audit_lean_tcb_tree_with_mode, LeanKernelMode, LeanTcbRole, LeanTcbTreeReport,
    LeanVerificationTier, DEFAULT_MAX_LEAN_FILES,
};

use crate::cli::Cli;
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "LeanTcb")]
pub(crate) struct Args {
    /// Optional `.lean` files or directories relative to `--root` (defaults to the whole repository).
    pub(crate) targets: Vec<PathBuf>,

    /// Maximum `.lean` files to scan before marking `walk_incomplete = true`.
    #[arg(long, default_value_t = DEFAULT_MAX_LEAN_FILES)]
    pub(crate) max_files: usize,

    /// Exit non-zero if any `.lean` file has a TCB soundness hole or if `walk_incomplete` is true.
    #[arg(long, default_value_t = false)]
    pub(crate) strict: bool,

    /// Run `lean` (`#print axioms`) and `lean4checker --fresh` when installed on `PATH`.
    #[arg(long, default_value_t = false)]
    pub(crate) kernel: bool,

    /// Require both `lean` kernel verification and `lean4checker --fresh` replay; fail closed if unavailable or unsound.
    #[arg(long, default_value_t = false)]
    pub(crate) require_kernel: bool,
}

fn tier_str(tier: LeanVerificationTier) -> &'static str {
    match tier {
        LeanVerificationTier::LexicalOnly => "lexical_only",
        LeanVerificationTier::LeanKernel => "lean_kernel",
        LeanVerificationTier::KernelAndLean4checker => "kernel_and_lean4checker",
    }
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let root = cli.root_hint();
    let mode = if args.require_kernel {
        LeanKernelMode::Require
    } else if args.kernel {
        LeanKernelMode::Auto
    } else {
        LeanKernelMode::Off
    };
    let report = audit_lean_tcb_tree_with_mode(&root, &args.targets, args.max_files, mode)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        render_human(&report);
    }
    let gate_error = if args.require_kernel
        && (!report.sound
            || report.verification_tier != LeanVerificationTier::KernelAndLean4checker)
    {
        Some(format!(
            "Lean 4 kernel + lean4checker TCB gate failed: tier={}, unsound_files={}, walk_incomplete={}, note={}",
            tier_str(report.verification_tier),
            report.unsound_files,
            report.walk_incomplete,
            report.kernel_note.as_deref().unwrap_or("none")
        ))
    } else if args.strict && !report.sound {
        Some(format!(
            "Lean 4 TCB boundary audit failed: {} unsound file(s), walk_incomplete={}",
            report.unsound_files, report.walk_incomplete
        ))
    } else {
        None
    };
    if let Some(msg) = gate_error {
        if cli.json {
            // `emit_json` already wrote the structured `LeanTcbTreeReport` line
            // to stdout; record the diagnostic on stderr and exit 1 directly so
            // `main` does not append a second JSON object to stdout.
            crate::diagnostic(format_args!("Error: {msg}"));
            std::process::exit(1);
        }
        bail!("{msg}");
    }
    Ok(())
}

fn render_human(report: &LeanTcbTreeReport) {
    for file in &report.files {
        let status = if file.sound_tcb { "SOUND" } else { "UNSOUND" };
        outln!(
            "{status:<8} {} [{}]",
            file.path,
            tier_str(file.verification_tier)
        );
        for decl in &file.declarations {
            if matches!(
                decl.role,
                LeanTcbRole::TrustedAxiom | LeanTcbRole::ExternHook
            ) {
                outln!(
                    "  line {}: {:?} {} ({})",
                    decl.start_line,
                    decl.role,
                    decl.qualified_name,
                    decl.keyword
                );
            }
        }
        for hole in &file.holes {
            let sym = hole.enclosing_symbol.as_deref().unwrap_or("<top-level>");
            outln!(
                "  line {}: {:?} `{}` in {}",
                hole.line,
                hole.kind,
                hole.token,
                sym
            );
        }
        if file.unread_bytes > 0 {
            outln!("  unread_bytes: {} (truncated at cap)", file.unread_bytes);
        }
    }
    for skip in &report.skipped {
        outln!("SKIPPED  {} ({})", skip.path, skip.reason);
    }
    if let Some(note) = &report.kernel_note {
        outln!("note: {note}");
    }
    outln!(
        "audited={} unsound={} walk_incomplete={} tier={} sound={}",
        report.files_audited,
        report.unsound_files,
        report.walk_incomplete,
        tier_str(report.verification_tier),
        report.sound
    );
}
