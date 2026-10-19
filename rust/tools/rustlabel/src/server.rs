//! Running rust-analyzer for a checkout: the prerequisites it needs, the one
//! configuration this tool starts it with, and the [`Locator`] that adapts the
//! workspace's language-server session to the labeller.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devmap_extract::subprocess::{self, Bounds};
use devmap_resolve::lsp::{
    find_server_binary, LspAnswer, LspDidNotRun, LspQuery, LspSession, LspSessionOptions,
    LSP_SERVERS,
};
use serde_json::json;

use crate::labels::Labels;
use crate::run::{label, Limits, LocateFailure, Locator, Outcome};

/// How rust-analyzer is run and how long each part of a run may take.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// One request's reply deadline.
    pub request_budget: Duration,
    /// How long loading the workspace (cargo metadata, build scripts, proc
    /// macros) may take before the run gives up.
    pub load_budget: Duration,
    /// The whole run, load included.
    pub overall_budget: Duration,
    /// Most definition requests.
    pub max_sites: usize,
    /// Label functions used as values as well as calls.
    pub references: bool,
    /// Where cargo builds for rust-analyzer. `None` is a fresh directory under
    /// the system temp dir, removed afterwards: the checkout is never written.
    pub target_dir: Option<PathBuf>,
    /// Keep cargo off the network, so a labelling run cannot change with the
    /// registry.
    pub offline: bool,
    /// Run build scripts, so generated code resolves.
    pub build_scripts: bool,
    /// Keep one line per site saying what became of it.
    pub trace: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            request_budget: Duration::from_secs(30),
            load_budget: Duration::from_secs(900),
            overall_budget: Duration::from_secs(4 * 3600),
            max_sites: 2_000_000,
            references: true,
            target_dir: None,
            offline: true,
            build_scripts: true,
            trace: false,
        }
    }
}

impl ServerConfig {
    /// A fixture-sized run: short bounds, so a hung server fails the test.
    pub fn for_tests() -> Self {
        Self {
            load_budget: Duration::from_secs(300),
            overall_budget: Duration::from_secs(600),
            max_sites: 100_000,
            ..Self::default()
        }
    }
}

/// The rust-analyzer that runs for `dir`.
///
/// `find_server_binary` refuses a server inside the repository, and follows
/// links to the real file. Installed through rustup, `rust-analyzer` is a link
/// to `rustup` itself, which decides what to be by the name it is started
/// under: started as `rustup` it prints its help and exits 1. So when the
/// resolved file is not named `rust-analyzer`, ask that rustup which binary the
/// toolchain in effect for `dir` ships, and run that.
pub fn resolve_rust_analyzer(dir: &Path) -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH");
    let found = find_server_binary("rust-analyzer", path.as_deref(), dir).map_err(|e| match e {
        LspDidNotRun::MissingBinary => "rust-analyzer is not on PATH".to_string(),
        other => format!("rust-analyzer was refused: {other:?}"),
    })?;
    if found.file_name().is_some_and(|n| n == "rust-analyzer") {
        return Ok(found);
    }
    let mut which = subprocess::session_command(&found);
    which.args(["which", "rust-analyzer"]).current_dir(dir);
    let done = subprocess::run_bounded(
        &mut which,
        Bounds {
            deadline: Duration::from_secs(30),
            stdout_cap: 4096,
            stderr_cap: 4096,
        },
    )
    .map_err(|e| format!("{} which rust-analyzer: {e}", found.display()))?;
    let real = PathBuf::from(done.stdout_lossy().trim());
    if !done.status.success() || !real.is_file() {
        return Err(format!(
            "{} is a link to {}, which is not rust-analyzer, and `which rust-analyzer` for {} failed: {}",
            "rust-analyzer",
            found.display(),
            dir.display(),
            done.stderr_trimmed()
        ));
    }
    Ok(real)
}

/// What a run needs installed.
pub struct Prerequisites;

impl Prerequisites {
    /// Check for rust-analyzer and for std sources, for the toolchain that
    /// runs in `dir`. Without std sources every call that resolves through a
    /// std type is unanswerable, so a run without them would label only part of
    /// the sites it appears to cover.
    pub fn check_in(dir: &Path) -> Result<(), String> {
        resolve_rust_analyzer(dir)?;
        let mut rustc = subprocess::session_command("rustc");
        rustc.args(["--print", "sysroot"]).current_dir(dir);
        let done = subprocess::run_bounded(
            &mut rustc,
            Bounds {
                deadline: Duration::from_secs(30),
                stdout_cap: 4096,
                stderr_cap: 4096,
            },
        )
        .map_err(|e| format!("rustc --print sysroot in {}: {e}", dir.display()))?;
        if !done.status.success() || done.stdout_truncated {
            return Err(format!(
                "rustc --print sysroot in {} failed: {}",
                dir.display(),
                done.stderr_trimmed()
            ));
        }
        let sysroot = PathBuf::from(done.stdout_lossy().trim());
        if !sysroot.join("lib/rustlib/src/rust/library/core").is_dir() {
            return Err(format!(
                "the rust-src component is missing from the toolchain that runs in {} (sysroot {}); install it with `rustup component add rust-src`",
                dir.display(),
                sysroot.display()
            ));
        }
        Ok(())
    }
}

/// A running rust-analyzer as a [`Locator`]. A reply the server refuses is
/// retried twice (rust-analyzer cancels a request when the workspace changes
/// under it); a session that ended is fatal.
struct RustAnalyzer {
    session: LspSession,
}

fn describe(error: &LspDidNotRun) -> String {
    let text = format!("{error:?}");
    text.chars().take(200).collect()
}

impl Locator for RustAnalyzer {
    fn locate(
        &mut self,
        query: LspQuery,
        file: &str,
        source: &str,
        line: u32,
        character: u32,
    ) -> Result<LspAnswer, LocateFailure> {
        let mut last = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(200));
            }
            match self.session.query(query, file, source, line, character) {
                Ok(answer) => return Ok(answer),
                Err(error) if self.session.is_usable() => last = describe(&error),
                Err(error) => return Err(LocateFailure::Fatal(describe(&error))),
            }
        }
        Err(LocateFailure::Request(last))
    }
}

fn initialization_options(config: &ServerConfig, target_dir: &Path) -> serde_json::Value {
    let mut cargo = json!({
        "targetDir": target_dir,
        "buildScripts": { "enable": config.build_scripts },
    });
    if config.offline {
        cargo["extraEnv"] = json!({ "CARGO_NET_OFFLINE": "true" });
    }
    json!({
        "cargo": cargo,
        "checkOnSave": false,
        "diagnostics": { "enable": false },
        "cachePriming": { "enable": false },
    })
}

/// Label every Rust file DevMap would index under `root`.
///
/// Fails, rather than returning partial labels, when rust-analyzer cannot be
/// started, cannot load the workspace, or reports `health: error`. A session
/// that dies mid-run still returns: its labels are real, and the report says
/// how many sites went unknown and why.
pub fn label_checkout(root: &Path, config: &ServerConfig) -> Result<Outcome, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let (sources, _) = devmap_extract::collect_sources_with_report(&root)
        .map_err(|e| format!("reading {}: {e}", root.display()))?;
    let files: Vec<(String, String)> = sources
        .into_iter()
        .filter(|(path, _)| path.ends_with(".rs"))
        .collect();
    if files.is_empty() {
        return Err(format!(
            "{} holds no Rust source DevMap indexes",
            root.display()
        ));
    }
    let binary = resolve_rust_analyzer(&root)?;
    let spec = LSP_SERVERS
        .iter()
        .find(|s| s.binary == "rust-analyzer")
        .ok_or("devmap-resolve has no rust-analyzer server spec")?;

    let started = Instant::now();
    let deadline = started + config.overall_budget;
    let (target_dir, owned_target) = match &config.target_dir {
        Some(dir) => (dir.clone(), false),
        None => (
            std::env::temp_dir().join(format!("rustlabel-target-{}", std::process::id())),
            true,
        ),
    };
    let result = (|| {
        let session = LspSession::start(
            spec,
            &binary,
            &root,
            deadline,
            &LspSessionOptions {
                request_budget: config.request_budget,
                initialization_options: Some(initialization_options(config, &target_dir)),
                server_status: true,
            },
        )
        .map_err(|e| format!("starting rust-analyzer: {e:?}"))?;
        let mut ra = RustAnalyzer { session };
        let version = ra.session.server_version().map(str::to_string);
        let loaded = ra
            .session
            .wait_quiescent((started + config.load_budget).min(deadline));
        let health = match loaded {
            Ok(health) => health,
            Err(e) => {
                ra.session.shutdown();
                return Err(format!(
                    "rust-analyzer did not finish loading {} within {:?}: {e:?}",
                    root.display(),
                    config.load_budget
                ));
            }
        };
        if health.health == "error" {
            ra.session.shutdown();
            return Err(format!(
                "rust-analyzer could not load the workspace at {}: {}",
                root.display(),
                health.message.unwrap_or_default()
            ));
        }
        let limits = Limits {
            max_sites: config.max_sites,
            deadline,
            references: config.references,
            trace: config.trace,
        };
        let mut outcome = label(&files, &mut ra, &limits);
        ra.session.shutdown();
        outcome.report.server = version;
        outcome.report.server_health = Some(match health.message {
            Some(message) => format!("{}: {message}", health.health),
            None => health.health,
        });
        Ok(outcome)
    })();
    if owned_target {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
    result
}

/// `labels` with hand-labelled dead rows merged verbatim.
pub fn with_dead(mut labels: Labels, dead: Vec<crate::labels::DeadLabel>) -> Labels {
    labels.dead = dead;
    labels
}
