//! Remote `devmap` store/CSR cache CLI (`devmap cache pack|unpack|status|push|pull`).
//!
//! Security & fail-closed invariants:
//! - `GITPULSE_RELAY_TOKEN` is read exclusively from the environment (never argv)
//!   and is never logged or echoed. For HTTPS requests via `curl`, the bearer
//!   header travels through a mode-0600 temporary `--config` file deleted on drop.
//! - Plain `http://` is permitted only for loopback hosts (`127.0.0.1`,
//!   `localhost`, `[::1]`) for local development and tests; remote endpoints
//!   must use `https://`.
//! - `unpack` fails closed on schema version mismatch, format version mismatch,
//!   SHA-256 mismatch, undeclared archive files, symlinks, hardlinks, `..` or
//!   absolute paths, or archives exceeding 256 MiB (compressed or decompressed).
//! - Unpacked SQLite stores are staged beside the target, verified, rebound to
//!   the target repository root, and swapped atomically under the store's writer
//!   lock.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::Subcommand;
use devmap_extract::paths::{csr_path_for_store, state_dir};
use devmap_store::seed::{self, SQLITE_SIDECARS};
use devmap_store::{current_git_head, Store, CURRENT_SCHEMA_VERSION};

use crate::cache_archive::{
    pack_tar_gz, unpack_tar_gz_strict, validate_archive_entry_path, CACHE_FORMAT_VERSION,
    MAX_ARCHIVE_BYTES,
};
use crate::cli::{root_parser, Cli};
use crate::output::{emit_json, ensure_parent};
use crate::sha256::sha256_hex;

/// Environment variable carrying the GitPulse Relay bearer token (never argv).
pub const RELAY_TOKEN_ENV: &str = "GITPULSE_RELAY_TOKEN";
/// Optional environment variable carrying a shared team HMAC-SHA256 signing key
/// for verifying cache manifest provenance on unpack/pull (C8).
pub const CACHE_SIGNING_KEY_ENV: &str = "DEVMAP_CACHE_SIGNING_KEY";

const MANIFEST_ENTRY_NAME: &str = "manifest.json";
const DB_ENTRY_NAME: &str = "devmap.sqlite";
const CSR_ENTRY_NAME: &str = "devmap.sqlite.csr";
const ARTIFACTS_ENTRY_NAME: &str = "devmap.sqlite.artifacts.json";
const CACHE_STATE_FILE: &str = "cache_state.json";

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(clap::Args)]
#[group(id = "Cache")]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) action: CacheAction,
}

#[derive(Subcommand)]
pub(crate) enum CacheAction {
    /// Pack the local store and CSR/extract-cache sidecars into a deterministic .tar.gz.
    Pack {
        /// Destination archive path (.tar.gz).
        #[arg(long, value_parser = root_parser())]
        out: PathBuf,
        /// Optional explicit 64-hex repository hash (defaults to SHA-256 of canonical repo root).
        #[arg(long)]
        repo_hash: Option<String>,
        /// Include indexed string literals (`payload_literals` / `generation_literals`) in the archive.
        /// By default literals are stripped so possible secrets in source literals are never exported.
        #[arg(long, default_value_t = false)]
        include_literals: bool,
    },
    /// Verify and atomically unpack a cache .tar.gz into this repository's store.
    Unpack {
        /// Source archive path (.tar.gz).
        #[arg(long = "in", value_parser = root_parser())]
        input: PathBuf,
        /// Optional explicit 64-hex repository hash expected in manifest.json.
        #[arg(long)]
        repo_hash: Option<String>,
        /// Allow unpacking a cache whose manifest repo_root_hash differs from this repository.
        #[arg(long, default_value_t = false)]
        allow_foreign_repo: bool,
        /// Refuse unpacking unless manifest git_head exactly matches this checkout's HEAD.
        #[arg(long, default_value_t = false)]
        require_git_head: bool,
    },
    /// Report local store cache readiness, schema version, and relay auth availability.
    Status {
        /// Optional explicit 64-hex repository hash.
        #[arg(long)]
        repo_hash: Option<String>,
    },
    /// Pack and push the local cache bundle via GitPulse Relay presigned R2 URL.
    Push {
        /// GitPulse Relay base URL (https://..., or http://127.0.0.1:... for local tests).
        #[arg(long)]
        relay_url: String,
        /// Optional Space ID for team-scoped cache sharing (defaults to account scope).
        #[arg(long)]
        space_id: Option<String>,
        /// Optional explicit 64-hex repository hash.
        #[arg(long)]
        repo_hash: Option<String>,
        /// Explicitly opt in to uploading indexed string literals in plaintext (stripped by default).
        #[arg(long, default_value_t = false)]
        include_literals: bool,
    },
    /// Pull, verify SHA-256, and atomically unpack a cache bundle via GitPulse Relay.
    Pull {
        /// GitPulse Relay base URL (https://..., or http://127.0.0.1:... for local tests).
        #[arg(long)]
        relay_url: String,
        /// Optional Space ID for team-scoped cache sharing (defaults to account scope).
        #[arg(long)]
        space_id: Option<String>,
        /// Expected 64-hex SHA-256 digest of the remote cache archive.
        #[arg(long)]
        digest: Option<String>,
        /// Optional explicit 64-hex repository hash.
        #[arg(long)]
        repo_hash: Option<String>,
        /// Allow unpacking a cache whose manifest repo_root_hash differs from this repository.
        #[arg(long, default_value_t = false)]
        allow_foreign_repo: bool,
        /// Refuse unpacking unless manifest git_head exactly matches this checkout's HEAD.
        #[arg(long, default_value_t = false)]
        require_git_head: bool,
    },
}

/// Manifest embedded inside every `devmap cache` archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheManifest {
    pub format_version: u32,
    pub schema_version: i32,
    pub extraction_schema_version: String,
    pub repo_root_hash: String,
    pub git_head: String,
    pub entries: Vec<CacheManifestEntry>,
    pub manifest_hmac: Option<String>,
}

impl CacheManifest {
    fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>> {
        let entries: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "path": e.path,
                    "size": e.size,
                    "sha256": e.sha256,
                })
            })
            .collect();
        let val = serde_json::json!({
            "format_version": self.format_version,
            "schema_version": self.schema_version,
            "extraction_schema_version": self.extraction_schema_version,
            "repo_root_hash": self.repo_root_hash,
            "git_head": self.git_head,
            "entries": entries,
        });
        Ok(serde_json::to_vec(&val)?)
    }

    fn to_json_value(&self) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "path": e.path,
                    "size": e.size,
                    "sha256": e.sha256,
                })
            })
            .collect();
        let mut obj = serde_json::Map::new();
        obj.insert(
            "format_version".into(),
            serde_json::json!(self.format_version),
        );
        obj.insert(
            "schema_version".into(),
            serde_json::json!(self.schema_version),
        );
        obj.insert(
            "extraction_schema_version".into(),
            serde_json::json!(self.extraction_schema_version),
        );
        obj.insert(
            "repo_root_hash".into(),
            serde_json::json!(self.repo_root_hash),
        );
        obj.insert("git_head".into(), serde_json::json!(self.git_head));
        obj.insert("entries".into(), serde_json::Value::Array(entries));
        if let Some(mac) = &self.manifest_hmac {
            obj.insert("manifest_hmac".into(), serde_json::json!(mac));
        }
        serde_json::Value::Object(obj)
    }

    fn from_json_slice(bytes: &[u8]) -> Result<Self> {
        let v: serde_json::Value =
            serde_json::from_slice(bytes).context("cache archive manifest.json is malformed")?;
        let obj = v
            .as_object()
            .ok_or_else(|| anyhow!("manifest.json must be a JSON object"))?;
        let format_version = obj
            .get("format_version")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| anyhow!("manifest.json missing integer format_version"))?
            as u32;
        let schema_version = obj
            .get("schema_version")
            .and_then(|x| x.as_i64())
            .ok_or_else(|| anyhow!("manifest.json missing integer schema_version"))?
            as i32;
        let extraction_schema_version = obj
            .get("extraction_schema_version")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string();
        let repo_root_hash = obj
            .get("repo_root_hash")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string();
        let git_head = obj
            .get("git_head")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string();
        let manifest_hmac = obj
            .get("manifest_hmac")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        let raw_entries = obj
            .get("entries")
            .and_then(|x| x.as_array())
            .ok_or_else(|| anyhow!("manifest.json missing entries array"))?;
        let mut entries = Vec::with_capacity(raw_entries.len());
        for item in raw_entries {
            let eobj = item
                .as_object()
                .ok_or_else(|| anyhow!("manifest.json entry must be an object"))?;
            let path = eobj
                .get("path")
                .and_then(|x| x.as_str())
                .ok_or_else(|| anyhow!("manifest entry missing path"))?
                .to_string();
            let size = eobj
                .get("size")
                .and_then(|x| x.as_u64())
                .ok_or_else(|| anyhow!("manifest entry missing size"))?;
            let sha256 = eobj
                .get("sha256")
                .and_then(|x| x.as_str())
                .ok_or_else(|| anyhow!("manifest entry missing sha256"))?
                .to_string();
            entries.push(CacheManifestEntry { path, size, sha256 });
        }
        Ok(Self {
            format_version,
            schema_version,
            extraction_schema_version,
            repo_root_hash,
            git_head,
            entries,
            manifest_hmac,
        })
    }
}

/// Pure-Rust RFC 2104 HMAC-SHA256 using workspace `sha2` (zero extra crates).
fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        k[..32].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut i_key_pad = [0x36u8; BLOCK];
    let mut o_key_pad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        i_key_pad[i] ^= k[i];
        o_key_pad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(i_key_pad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(o_key_pad);
    outer.update(inner_hash);
    format!("{:x}", outer.finalize())
}

fn constant_time_eq_hex(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Integrity record for one payload file inside a cache archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheManifestEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
struct LocalCacheState {
    last_digest: String,
    schema_version: i32,
    repo_root_hash: String,
}

#[derive(Debug, Clone)]
struct PresignRequest {
    scope: String,
    space_id: Option<String>,
    repo_hash: String,
    schema_version: i32,
    digest: Option<String>,
    action: String,
    bytes: Option<u64>,
}

impl PresignRequest {
    fn to_json_bytes(&self) -> Result<Vec<u8>> {
        let mut map = serde_json::Map::new();
        map.insert(
            "scope".into(),
            serde_json::Value::String(self.scope.clone()),
        );
        if let Some(sid) = &self.space_id {
            map.insert("space_id".into(), serde_json::Value::String(sid.clone()));
        }
        map.insert(
            "repo_hash".into(),
            serde_json::Value::String(self.repo_hash.clone()),
        );
        map.insert(
            "schema_version".into(),
            serde_json::Value::Number(self.schema_version.into()),
        );
        if let Some(d) = &self.digest {
            map.insert("digest".into(), serde_json::Value::String(d.clone()));
        }
        map.insert(
            "action".into(),
            serde_json::Value::String(self.action.clone()),
        );
        if let Some(b) = self.bytes {
            map.insert("bytes".into(), serde_json::Value::Number(b.into()));
        }
        Ok(serde_json::to_vec(&serde_json::Value::Object(map))?)
    }
}

#[derive(Debug, Clone)]
struct PresignResponse {
    url: String,
    method: Option<String>,
    headers: BTreeMap<String, String>,
    digest: Option<String>,
    sha256: Option<String>,
}

impl PresignResponse {
    fn from_json_slice(bytes: &[u8]) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_slice(bytes)
            .context("invalid JSON response from relay presign endpoint")?;
        let obj = v
            .as_object()
            .ok_or_else(|| anyhow!("presign response must be a JSON object"))?;
        let url = obj
            .get("url")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("presign response missing url"))?
            .to_string();
        let method = obj
            .get("method")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        let mut headers = BTreeMap::new();
        if let Some(hmap) = obj.get("headers").and_then(|x| x.as_object()) {
            for (k, val) in hmap {
                if let Some(s) = val.as_str() {
                    headers.insert(k.clone(), s.to_string());
                }
            }
        }
        let digest = obj
            .get("digest")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        let sha256 = obj
            .get("sha256")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        Ok(Self {
            url,
            method,
            headers,
            digest,
            sha256,
        })
    }
}

/// Unpack provenance options controlling `repo_root_hash` and `git_head` checks.
#[derive(Debug, Clone, Default)]
pub struct UnpackOptions {
    pub expected_repo_hash: Option<String>,
    pub allow_foreign_repo: bool,
    pub require_git_head: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> Result<()> {
    let root = resolve_repo_root(cli)?;
    let db_path = cli.db();
    match &args.action {
        CacheAction::Pack {
            out,
            repo_hash,
            include_literals,
        } => {
            let repo_root_hash = resolve_repo_hash(&root, repo_hash.as_deref())?;
            let (archive_bytes, manifest, archive_sha256) =
                pack_store_to_bytes_opts(&root, &db_path, &repo_root_hash, *include_literals)?;
            ensure_parent(out)?;
            write_file_atomic(out, &archive_bytes)?;
            let payload = serde_json::json!({
                "action": "pack",
                "out": out,
                "bytes": archive_bytes.len(),
                "sha256": archive_sha256,
                "schema_version": manifest.schema_version,
                "format_version": manifest.format_version,
                "repo_root_hash": manifest.repo_root_hash,
                "git_head": manifest.git_head,
                "literals_stripped": !include_literals,
                "entries": manifest.to_json_value()["entries"],
            });
            if cli.json {
                emit_json(cli, &payload)?;
            } else {
                outln!(
                    "packed {} ({} bytes, sha256 {}, {} entries, schema {})",
                    out.display(),
                    archive_bytes.len(),
                    &archive_sha256[..12],
                    manifest.entries.len(),
                    manifest.schema_version
                );
            }
        }
        CacheAction::Unpack {
            input,
            repo_hash,
            allow_foreign_repo,
            require_git_head,
        } => {
            let meta = std::fs::metadata(input)
                .with_context(|| format!("cannot inspect cache archive {}", input.display()))?;
            ensure!(
                meta.is_file(),
                "cache archive {} is not a regular file",
                input.display()
            );
            ensure!(
                meta.len() <= MAX_ARCHIVE_BYTES as u64,
                "cache archive {} ({} bytes) exceeds maximum allowed size of {} bytes",
                input.display(),
                meta.len(),
                MAX_ARCHIVE_BYTES
            );
            let expected_repo_hash = Some(resolve_repo_hash(&root, repo_hash.as_deref())?);
            let archive_bytes = std::fs::read(input)
                .with_context(|| format!("cannot read cache archive {}", input.display()))?;
            let archive_sha256 = sha256_hex(&archive_bytes);
            let manifest = unpack_bytes_into_store_opts(
                &root,
                &db_path,
                &archive_bytes,
                &UnpackOptions {
                    expected_repo_hash,
                    allow_foreign_repo: *allow_foreign_repo,
                    require_git_head: *require_git_head,
                },
            )?;
            let local_head = current_git_head(&root).unwrap_or_default();
            let git_head_matches = !local_head.is_empty() && local_head == manifest.git_head;
            let payload = serde_json::json!({
                "action": "unpack",
                "in": input,
                "db_path": db_path,
                "sha256": archive_sha256,
                "schema_version": manifest.schema_version,
                "format_version": manifest.format_version,
                "repo_root_hash": manifest.repo_root_hash,
                "git_head": manifest.git_head,
                "git_head_matches": git_head_matches,
                "entries_restored": manifest.entries.len(),
            });
            if cli.json {
                emit_json(cli, &payload)?;
            } else {
                outln!(
                    "unpacked {} into {} (schema {}, {} entries, head_match={})",
                    input.display(),
                    db_path.display(),
                    manifest.schema_version,
                    manifest.entries.len(),
                    git_head_matches
                );
            }
        }
        CacheAction::Status { repo_hash } => {
            let repo_root_hash = resolve_repo_hash(&root, repo_hash.as_deref())?;
            let git_head = current_git_head(&root).unwrap_or_default();
            let db_meta = std::fs::symlink_metadata(&db_path).ok();
            let db_present = db_meta.as_ref().is_some_and(|m| m.is_file());
            let db_bytes = db_meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let csr_path = csr_path_for_store(&db_path);
            let csr_meta = std::fs::symlink_metadata(&csr_path).ok();
            let csr_present = csr_meta.as_ref().is_some_and(|m| m.is_file());
            let csr_bytes = csr_meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let generation = if db_present {
                Store::open_existing(&db_path)
                    .ok()
                    .flatten()
                    .and_then(|s| s.status(&db_path.display().to_string()).ok())
                    .and_then(|st| st.latest_generation)
            } else {
                None
            };
            let relay_token_present = std::env::var(RELAY_TOKEN_ENV)
                .ok()
                .is_some_and(|v| !v.trim().is_empty());
            let last_state = read_local_cache_state(&root);
            let payload = serde_json::json!({
                "db_path": db_path,
                "db_present": db_present,
                "db_bytes": db_bytes,
                "csr_present": csr_present,
                "csr_bytes": csr_bytes,
                "schema_version": CURRENT_SCHEMA_VERSION,
                "format_version": CACHE_FORMAT_VERSION,
                "repo_root_hash": repo_root_hash,
                "git_head": git_head,
                "generation": generation,
                "relay_token_configured": relay_token_present,
                "last_pushed_digest": last_state.map(|s| s.last_digest),
            });
            if cli.json {
                emit_json(cli, &payload)?;
            } else {
                outln!(
                    "db={} present={} ({} bytes) csr={} ({} bytes) schema={} repo_hash={} relay_token={}",
                    db_path.display(),
                    db_present,
                    db_bytes,
                    csr_present,
                    csr_bytes,
                    CURRENT_SCHEMA_VERSION,
                    &repo_root_hash[..12],
                    if relay_token_present { "set" } else { "unset" }
                );
            }
        }
        CacheAction::Push {
            relay_url,
            space_id,
            repo_hash,
            include_literals,
        } => {
            let token = read_relay_token()?;
            let repo_root_hash = resolve_repo_hash(&root, repo_hash.as_deref())?;
            let report = push_cache_opts(
                &root,
                &db_path,
                relay_url,
                space_id.as_deref(),
                &repo_root_hash,
                &token,
                *include_literals,
            )?;
            let payload = serde_json::json!({
                "action": "push",
                "scope": report.scope,
                "space_id": report.space_id,
                "repo_root_hash": report.repo_root_hash,
                "schema_version": report.schema_version,
                "digest": report.digest,
                "bytes": report.bytes,
                "entries": report.entries,
                "literals_stripped": !include_literals,
            });
            if cli.json {
                emit_json(cli, &payload)?;
            } else {
                outln!(
                    "pushed cache {} ({} bytes, schema {}, scope={})",
                    &report.digest[..12],
                    report.bytes,
                    report.schema_version,
                    report.scope
                );
            }
        }
        CacheAction::Pull {
            relay_url,
            space_id,
            digest,
            repo_hash,
            allow_foreign_repo,
            require_git_head,
        } => {
            let token = read_relay_token()?;
            let repo_root_hash = resolve_repo_hash(&root, repo_hash.as_deref())?;
            let manifest_and_digest = pull_cache_opts(
                &root,
                &db_path,
                relay_url,
                space_id.as_deref(),
                digest.as_deref(),
                &repo_root_hash,
                &token,
                &UnpackOptions {
                    expected_repo_hash: Some(repo_root_hash.clone()),
                    allow_foreign_repo: *allow_foreign_repo,
                    require_git_head: *require_git_head,
                },
            )?;
            let payload = serde_json::json!({
                "action": "pull",
                "db_path": db_path,
                "digest": manifest_and_digest.1,
                "schema_version": manifest_and_digest.0.schema_version,
                "format_version": manifest_and_digest.0.format_version,
                "repo_root_hash": manifest_and_digest.0.repo_root_hash,
                "git_head": manifest_and_digest.0.git_head,
                "entries_restored": manifest_and_digest.0.entries.len(),
            });
            if cli.json {
                emit_json(cli, &payload)?;
            } else {
                outln!(
                    "pulled and unpacked cache {} into {} (schema {}, {} entries)",
                    &manifest_and_digest.1[..12],
                    db_path.display(),
                    manifest_and_digest.0.schema_version,
                    manifest_and_digest.0.entries.len()
                );
            }
        }
    }
    Ok(())
}

fn resolve_repo_root(cli: &Cli) -> Result<PathBuf> {
    let hint = cli.root_hint();
    Ok(hint.canonicalize().unwrap_or(hint))
}

fn validate_hex64(value: &str, label: &str) -> Result<String> {
    let trimmed = value.trim().to_ascii_lowercase();
    ensure!(
        trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()),
        "{label} must be a 64-character hex SHA-256 digest"
    );
    Ok(trimmed)
}

fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn normalize_remote_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/').trim_end_matches(".git");
    trimmed.to_ascii_lowercase()
}

/// Derive a stable, machine-independent repository identity hash (C8):
/// uses canonical `remote.origin.url` + root commit SHA when inside a git
/// repository so teammates on different checkout paths produce the exact same
/// `repo_root_hash`, falling back to canonical path hash only for non-git trees.
fn resolve_repo_hash(root: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(h) = explicit {
        return validate_hex64(h, "--repo-hash");
    }
    let root_commit = git_stdout(root, &["rev-list", "--max-parents=0", "HEAD"])
        .and_then(|s| s.lines().next().map(str::trim).map(str::to_string))
        .filter(|s| !s.is_empty());
    let remote = git_stdout(root, &["config", "--get", "remote.origin.url"])
        .map(|u| normalize_remote_url(&u));
    if root_commit.is_some() || remote.is_some() {
        let identity = format!(
            "devmap-repo/v1\0{}\0{}",
            remote.unwrap_or_default(),
            root_commit.unwrap_or_default()
        );
        return Ok(sha256_hex(identity.as_bytes()));
    }
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    Ok(sha256_hex(canonical.to_string_lossy().as_bytes()))
}

fn validate_relay_token(raw: &str) -> Result<String> {
    let trimmed = raw.trim().to_string();
    ensure!(
        !trimmed.is_empty(),
        "{RELAY_TOKEN_ENV} environment variable is empty"
    );
    ensure!(
        !trimmed
            .bytes()
            .any(|b| b <= 0x1f || b == 0x7f || b == b'"' || b == b'\\'),
        "{RELAY_TOKEN_ENV} contains invalid control, quote, or backslash characters"
    );
    Ok(trimmed)
}

fn read_relay_token() -> Result<String> {
    let token = std::env::var(RELAY_TOKEN_ENV).map_err(|_| {
        anyhow!(
            "{RELAY_TOKEN_ENV} environment variable is required for remote cache push/pull (tokens are never accepted via CLI flags)"
        )
    })?;
    validate_relay_token(&token)
}

fn is_strict_loopback_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(rest) = authority.strip_prefix("[::1]") {
        return rest.is_empty()
            || rest
                .strip_prefix(':')
                .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if host != "127.0.0.1" && host != "localhost" {
        return false;
    }
    match port {
        None => true,
        Some(p) => !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()),
    }
}

fn validate_url_with_policy(raw: &str, label: &str, allow_loopback_http: bool) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    ensure!(!trimmed.is_empty(), "{label} must not be empty");
    ensure!(
        !trimmed.bytes().any(|b| b <= 0x20
            || b == 0x7f
            || b == b'"'
            || b == b'\\'
            || b == b'{'
            || b == b'}'),
        "{label} contains unsafe characters"
    );
    if trimmed.starts_with("https://") {
        return Ok(trimmed.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("http://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        ensure!(
            allow_loopback_http && is_strict_loopback_authority(authority),
            "{label} must use https:// (plain http:// is refused for remote or presigned URLs unless the relay itself is loopback)"
        );
        return Ok(trimmed.to_string());
    }
    bail!("{label} must start with https://")
}

fn validate_relay_or_presign_url(raw: &str, label: &str) -> Result<String> {
    validate_url_with_policy(raw, label, true)
}

fn is_forbidden_relay_header(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "host"
            | "authorization"
            | "proxy-authorization"
            | "cookie"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "expect"
            | "te"
            | "upgrade"
            | "keep-alive"
    )
}

fn next_unique_suffix() -> String {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{seq}", std::process::id())
}

struct FileCleanupGuard(PathBuf);
impl Drop for FileCleanupGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct DirCleanupGuard(PathBuf);
impl Drop for DirCleanupGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_file_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cache.tar.gz");
    let tmp = parent.join(format!(".{name}.tmp-{}", next_unique_suffix()));
    let _guard = FileCleanupGuard(tmp.clone());
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("cannot create staging file {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, target)
        .with_context(|| format!("cannot place {} atomically", target.display()))?;
    Ok(())
}

/// Pack a repository's SQLite store, CSR sidecar, and extract/resolution caches
/// into a deterministic `.tar.gz` byte vector + manifest + archive SHA-256.
/// Strips source string literals by default (pass `include_literals = true` to retain them).
/// Test-only shorthand; the `cache` command calls [`pack_store_to_bytes_opts`].
#[cfg(test)]
pub fn pack_store_to_bytes(
    root: &Path,
    db_path: &Path,
    repo_root_hash: &str,
) -> Result<(Vec<u8>, CacheManifest, String)> {
    pack_store_to_bytes_opts(root, db_path, repo_root_hash, false)
}

pub fn pack_store_to_bytes_opts(
    root: &Path,
    db_path: &Path,
    repo_root_hash: &str,
    include_literals: bool,
) -> Result<(Vec<u8>, CacheManifest, String)> {
    let db_meta = std::fs::symlink_metadata(db_path).with_context(|| {
        format!(
            "no devmap store at {} — run `devmap build` first",
            db_path.display()
        )
    })?;
    ensure!(
        !db_meta.file_type().is_symlink() && db_meta.is_file(),
        "refusing to pack {}: store must be a regular file, not a symlink",
        db_path.display()
    );

    // Stage a consistent WAL-healed snapshot so live writers cannot corrupt it.
    let staged_db = seed::sidecar(db_path, &format!(".cache-pack-{}", next_unique_suffix()));
    seed::discard_staged(&staged_db);
    let _staged_cleanup = FileCleanupGuard(staged_db.clone());
    seed::stage_seed_healing_wal(db_path, &staged_db)
        .with_context(|| format!("failed to snapshot store {}", db_path.display()))?;

    // Strip indexed string literals by default (C9) so raw source strings /
    // possible secrets are not exported into plaintext remote cache bundles.
    if !include_literals {
        seed::strip_staged_literals(&staged_db)?;
    }

    // Validate the snapshot opens cleanly at CURRENT_SCHEMA_VERSION
    // (Store::open_existing fails closed on UnsupportedSchema or corruption).
    {
        let opened = Store::open_existing(&staged_db)?
            .ok_or_else(|| anyhow!("staged store snapshot vanished"))?;
        let _ = opened.status(&staged_db.display().to_string())?;
    }

    let mut payload_entries: Vec<(String, Vec<u8>)> = Vec::new();
    let db_bytes = std::fs::read(&staged_db)
        .with_context(|| format!("cannot read staged snapshot {}", staged_db.display()))?;
    payload_entries.push((DB_ENTRY_NAME.to_string(), db_bytes));

    // Optional CSR sidecar (regular file only, never follow symlinks).
    let csr_path = csr_path_for_store(db_path);
    if let Ok(meta) = std::fs::symlink_metadata(&csr_path) {
        if meta.is_file() && !meta.file_type().is_symlink() {
            let bytes = std::fs::read(&csr_path)?;
            payload_entries.push((CSR_ENTRY_NAME.to_string(), bytes));
        }
    }

    // Optional artifacts sidecar (regular file only).
    let artifacts_path = seed::sidecar(db_path, ".artifacts.json");
    if let Ok(meta) = std::fs::symlink_metadata(&artifacts_path) {
        if meta.is_file() && !meta.file_type().is_symlink() {
            let bytes = std::fs::read(&artifacts_path)?;
            payload_entries.push((ARTIFACTS_ENTRY_NAME.to_string(), bytes));
        }
    }

    // Optional resolution-reuse-* cache files in the state dir (regular files only).
    let sdir = state_dir(root);
    if let Ok(read_dir) = std::fs::read_dir(&sdir) {
        let mut reuse_dirs: Vec<(String, PathBuf)> = Vec::new();
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(rest) = name.strip_prefix("resolution-reuse-") {
                if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                    let p = entry.path();
                    if let Ok(m) = std::fs::symlink_metadata(&p) {
                        if m.is_dir() && !m.file_type().is_symlink() {
                            reuse_dirs.push((name, p));
                        }
                    }
                }
            }
        }
        reuse_dirs.sort_by(|a, b| a.0.cmp(&b.0));
        for (dir_name, dir_path) in reuse_dirs {
            if let Ok(files) = std::fs::read_dir(&dir_path) {
                let mut child_files: Vec<(String, PathBuf)> = Vec::new();
                for f in files.flatten() {
                    let fname = f.file_name().to_string_lossy().into_owned();
                    let fpath = f.path();
                    if let Ok(m) = std::fs::symlink_metadata(&fpath) {
                        if m.is_file() && !m.file_type().is_symlink() {
                            let rel = format!("{dir_name}/{fname}");
                            if validate_archive_entry_path(&rel).is_ok() {
                                child_files.push((rel, fpath));
                            }
                        }
                    }
                }
                child_files.sort_by(|a, b| a.0.cmp(&b.0));
                for (rel, fpath) in child_files {
                    let bytes = std::fs::read(&fpath)?;
                    payload_entries.push((rel, bytes));
                }
            }
        }
    }

    payload_entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut total_bytes = 0usize;
    let mut manifest_entries = Vec::with_capacity(payload_entries.len());
    for (path, data) in &payload_entries {
        total_bytes = total_bytes
            .checked_add(data.len())
            .ok_or_else(|| anyhow!("cache payload size overflow"))?;
        ensure!(
            total_bytes <= MAX_ARCHIVE_BYTES,
            "cache payload ({total_bytes} bytes) exceeds maximum allowed size ({MAX_ARCHIVE_BYTES} bytes)"
        );
        manifest_entries.push(CacheManifestEntry {
            path: path.clone(),
            size: data.len() as u64,
            sha256: sha256_hex(data),
        });
    }

    let mut manifest = CacheManifest {
        format_version: CACHE_FORMAT_VERSION,
        schema_version: CURRENT_SCHEMA_VERSION,
        extraction_schema_version: devmap_extract::cache::EXTRACTION_SCHEMA_VERSION.to_string(),
        repo_root_hash: repo_root_hash.to_string(),
        git_head: current_git_head(root).unwrap_or_default(),
        entries: manifest_entries,
        manifest_hmac: None,
    };
    if let Ok(key) = std::env::var(CACHE_SIGNING_KEY_ENV) {
        let key = key.trim();
        if !key.is_empty() {
            let unsigned = manifest.canonical_unsigned_bytes()?;
            manifest.manifest_hmac = Some(hmac_sha256_hex(key.as_bytes(), &unsigned));
        }
    }

    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_json_value())?;
    let mut all_entries = Vec::with_capacity(payload_entries.len() + 1);
    all_entries.push((MANIFEST_ENTRY_NAME.to_string(), manifest_bytes));
    all_entries.extend(payload_entries);

    let gz_bytes = pack_tar_gz(&all_entries)?;
    ensure!(
        gz_bytes.len() <= MAX_ARCHIVE_BYTES,
        "compressed cache archive ({} bytes) exceeds maximum allowed size ({MAX_ARCHIVE_BYTES} bytes)",
        gz_bytes.len()
    );
    let archive_sha256 = sha256_hex(&gz_bytes);
    Ok((gz_bytes, manifest, archive_sha256))
}

fn is_allowed_payload_path(path: &str) -> bool {
    if matches!(path, DB_ENTRY_NAME | CSR_ENTRY_NAME | ARTIFACTS_ENTRY_NAME) {
        return true;
    }
    if let Some((dir, file)) = path.split_once('/') {
        if !file.contains('/') && !file.is_empty() {
            if let Some(rest) = dir.strip_prefix("resolution-reuse-") {
                return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit());
            }
        }
    }
    false
}

/// Test convenience wrapper for unpacking without repo-hash enforcement.
/// Production callers MUST use [`unpack_bytes_into_store_opts`] with
/// `allow_foreign_repo: false` by default (C8).
#[cfg(test)]
pub fn unpack_bytes_into_store(
    root: &Path,
    db_path: &Path,
    gz_bytes: &[u8],
) -> Result<CacheManifest> {
    unpack_bytes_into_store_opts(
        root,
        db_path,
        gz_bytes,
        &UnpackOptions {
            expected_repo_hash: None,
            allow_foreign_repo: true,
            require_git_head: false,
        },
    )
}

/// Strictly verify provenance (`repo_root_hash`, optional `git_head`), SQLite
/// defensive `PRAGMA quick_check(1)` + `cell_size_check`, and atomically unpack
/// a `.tar.gz` cache archive into `db_path`.
pub fn unpack_bytes_into_store_opts(
    root: &Path,
    db_path: &Path,
    gz_bytes: &[u8],
    opts: &UnpackOptions,
) -> Result<CacheManifest> {
    ensure!(
        gz_bytes.len() <= MAX_ARCHIVE_BYTES,
        "cache archive ({} bytes) exceeds maximum allowed size ({MAX_ARCHIVE_BYTES} bytes)",
        gz_bytes.len()
    );

    let extracted = unpack_tar_gz_strict(gz_bytes, MAX_ARCHIVE_BYTES)?;
    let mut map: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (path, bytes) in extracted {
        ensure!(
            map.insert(path.clone(), bytes).is_none(),
            "duplicate entry {path:?} in cache archive"
        );
    }

    let manifest_bytes = map
        .remove(MANIFEST_ENTRY_NAME)
        .ok_or_else(|| anyhow!("cache archive is missing required {MANIFEST_ENTRY_NAME}"))?;
    let manifest = CacheManifest::from_json_slice(&manifest_bytes)?;

    ensure!(
        manifest.format_version == CACHE_FORMAT_VERSION,
        "unsupported cache format_version {} (expected {CACHE_FORMAT_VERSION})",
        manifest.format_version
    );
    ensure!(
        manifest.schema_version == CURRENT_SCHEMA_VERSION,
        "cache schema_version {} does not match local store schema {CURRENT_SCHEMA_VERSION}",
        manifest.schema_version
    );
    ensure!(
        !manifest.entries.is_empty(),
        "cache manifest declares zero entries"
    );

    // Provenance verification (C8): refuse foreign repo stores unless explicitly allowed.
    if !opts.allow_foreign_repo {
        if let Some(expected_hash) = &opts.expected_repo_hash {
            let actual_hash = validate_hex64(&manifest.repo_root_hash, "manifest repo_root_hash")?;
            ensure!(
                &actual_hash == expected_hash,
                "cache manifest repo_root_hash ({actual_hash}) does not match target repository ({expected_hash}); pass --allow-foreign-repo to override"
            );
        }
    }
    if opts.require_git_head {
        let local_head = current_git_head(root).unwrap_or_default();
        ensure!(
            !local_head.is_empty()
                && !manifest.git_head.is_empty()
                && local_head == manifest.git_head,
            "cache manifest git_head ({:?}) does not match local checkout HEAD ({local_head:?})",
            manifest.git_head
        );
    }

    // Optional cryptographic provenance verification (C8): when DEVMAP_CACHE_SIGNING_KEY
    // is configured, require a valid constant-time HMAC-SHA256 over canonical manifest bytes.
    if let Ok(key) = std::env::var(CACHE_SIGNING_KEY_ENV) {
        let key = key.trim();
        if !key.is_empty() {
            let provided = manifest.manifest_hmac.as_deref().ok_or_else(|| {
                anyhow!("{CACHE_SIGNING_KEY_ENV} is set, but cache manifest is unsigned")
            })?;
            let unsigned = manifest.canonical_unsigned_bytes()?;
            let expected = hmac_sha256_hex(key.as_bytes(), &unsigned);
            ensure!(
                constant_time_eq_hex(provided, &expected),
                "cache manifest HMAC-SHA256 signature verification failed"
            );
        }
    }

    // Verify every declared entry exists, is allowed, matches size and SHA-256,
    // and that zero undeclared files exist in the archive.
    let mut verified_payloads: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for entry in &manifest.entries {
        validate_archive_entry_path(&entry.path)?;
        ensure!(
            is_allowed_payload_path(&entry.path),
            "refusing unexpected file path {:?} in cache manifest",
            entry.path
        );
        let expected_hex = validate_hex64(&entry.sha256, "manifest entry sha256")?;
        let data = map
            .remove(&entry.path)
            .ok_or_else(|| anyhow!("cache archive missing declared entry {:?}", entry.path))?;
        ensure!(
            data.len() as u64 == entry.size,
            "size mismatch for {:?}: manifest declared {}, archive holds {}",
            entry.path,
            entry.size,
            data.len()
        );
        let actual_hex = sha256_hex(&data);
        ensure!(
            actual_hex == expected_hex,
            "SHA-256 digest mismatch for {:?}: expected {expected_hex}, got {actual_hex}",
            entry.path
        );
        ensure!(
            verified_payloads.insert(entry.path.clone(), data).is_none(),
            "duplicate entry {:?} in cache manifest",
            entry.path
        );
    }

    if let Some((extra, _)) = map.into_iter().next() {
        bail!("refusing undeclared extra file {extra:?} in cache archive");
    }

    let db_bytes = verified_payloads
        .remove(DB_ENTRY_NAME)
        .ok_or_else(|| anyhow!("cache manifest is missing required {DB_ENTRY_NAME} entry"))?;

    ensure_parent(db_path)?;
    // Refuse if target store or its parent is a symlink.
    if let Ok(meta) = std::fs::symlink_metadata(db_path) {
        ensure!(
            !meta.file_type().is_symlink(),
            "refusing to unpack onto symlinked store path {}",
            db_path.display()
        );
    }

    let stage_dir = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".cache-unpack-{}", next_unique_suffix()));
    std::fs::create_dir_all(&stage_dir)?;
    let _stage_cleanup = DirCleanupGuard(stage_dir.clone());

    let staged_db = stage_dir.join(DB_ENTRY_NAME);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_db)?;
        f.write_all(&db_bytes)?;
        f.sync_all()?;
    }

    // Verify staged SQLite integrity under SQLITE_DBCONFIG_DEFENSIVE +
    // PRAGMA trusted_schema = OFF + cell_size_check = ON + quick_check(1) (C8),
    // then verify schema version via Store::open_existing before rebinding.
    seed::verify_staged_integrity(&staged_db)?;
    {
        let opened = Store::open_existing(&staged_db)?
            .ok_or_else(|| anyhow!("unpacked SQLite store could not be opened"))?;
        let _ = opened.status(&staged_db.display().to_string())?;
    }
    seed::rebind_staged(&staged_db, root)
        .context("failed to rebind unpacked cache store to local repository root")?;

    // Optional CSR & artifacts staged beside DB.
    let staged_csr = if let Some(csr_bytes) = verified_payloads.remove(CSR_ENTRY_NAME) {
        let p = stage_dir.join(CSR_ENTRY_NAME);
        std::fs::write(&p, &csr_bytes)?;
        Some(p)
    } else {
        None
    };
    let staged_artifacts = if let Some(art_bytes) = verified_payloads.remove(ARTIFACTS_ENTRY_NAME) {
        let p = stage_dir.join(ARTIFACTS_ENTRY_NAME);
        std::fs::write(&p, &art_bytes)?;
        Some(p)
    } else {
        None
    };

    // Acquire store writer lock before replacing live store files.
    let _writer_lock = Store::lock_writer_at(db_path, Store::WRITER_LOCK_WAIT)?;
    for suffix in SQLITE_SIDECARS {
        let sc = seed::sidecar(db_path, suffix);
        let _ = std::fs::remove_file(sc);
    }

    std::fs::rename(&staged_db, db_path)
        .with_context(|| format!("failed to atomically replace {}", db_path.display()))?;

    let target_csr = csr_path_for_store(db_path);
    match staged_csr {
        Some(src) => {
            std::fs::rename(&src, &target_csr)
                .with_context(|| format!("failed to place {}", target_csr.display()))?;
        }
        None => {
            let _ = std::fs::remove_file(&target_csr);
        }
    }

    let target_artifacts = seed::sidecar(db_path, ".artifacts.json");
    match staged_artifacts {
        Some(src) => {
            std::fs::rename(&src, &target_artifacts)
                .with_context(|| format!("failed to place {}", target_artifacts.display()))?;
        }
        None => {
            let _ = std::fs::remove_file(&target_artifacts);
        }
    }

    // Place any resolution-reuse-* cache files safely under state_dir(root).
    let sdir = state_dir(root);
    for (rel, bytes) in verified_payloads {
        if let Some((dir_name, file_name)) = rel.split_once('/') {
            let target_dir = sdir.join(dir_name);
            if let Ok(m) = std::fs::symlink_metadata(&target_dir) {
                ensure!(
                    !m.file_type().is_symlink() && m.is_dir(),
                    "refusing symlinked cache directory {}",
                    target_dir.display()
                );
            } else {
                std::fs::create_dir_all(&target_dir)?;
            }
            let target_file = target_dir.join(file_name);
            if let Ok(m) = std::fs::symlink_metadata(&target_file) {
                ensure!(
                    !m.file_type().is_symlink(),
                    "refusing symlinked cache file {}",
                    target_file.display()
                );
            }
            write_file_atomic(&target_file, &bytes)?;
        }
    }

    Ok(manifest)
}

#[derive(Debug, Clone)]
pub struct PushReport {
    pub scope: String,
    pub space_id: Option<String>,
    pub repo_root_hash: String,
    pub schema_version: i32,
    pub digest: String,
    pub bytes: usize,
    pub entries: usize,
}

/// Pack, request a presigned upload URL from GitPulse Relay, and upload bytes.
/// Test-only shorthand; the `cache push` command calls [`push_cache_opts`].
#[cfg(test)]
pub fn push_cache(
    root: &Path,
    db_path: &Path,
    relay_url: &str,
    space_id: Option<&str>,
    repo_root_hash: &str,
    bearer_token: &str,
) -> Result<PushReport> {
    push_cache_opts(
        root,
        db_path,
        relay_url,
        space_id,
        repo_root_hash,
        bearer_token,
        false,
    )
}

pub fn push_cache_opts(
    root: &Path,
    db_path: &Path,
    relay_url: &str,
    space_id: Option<&str>,
    repo_root_hash: &str,
    bearer_token: &str,
    include_literals: bool,
) -> Result<PushReport> {
    let token = validate_relay_token(bearer_token)?;
    let base = validate_relay_or_presign_url(relay_url, "--relay-url")?;
    let relay_is_loopback = base.starts_with("http://");
    let (archive_bytes, manifest, archive_sha256) =
        pack_store_to_bytes_opts(root, db_path, repo_root_hash, include_literals)?;
    let scope = if space_id.is_some() {
        "space".to_string()
    } else {
        "account".to_string()
    };
    let req = PresignRequest {
        scope: scope.clone(),
        space_id: space_id.map(str::to_string),
        repo_hash: repo_root_hash.to_string(),
        schema_version: manifest.schema_version,
        digest: Some(archive_sha256.clone()),
        action: "upload".to_string(),
        bytes: Some(archive_bytes.len() as u64),
    };
    let presign_url = format!("{base}/v1/devmap-cache/presign");
    let presign = request_presign(&presign_url, &token, &req)?;
    // C7: presigned URL MUST use https:// unless --relay-url itself is loopback http://.
    let upload_url =
        validate_url_with_policy(&presign.url, "presigned upload URL", relay_is_loopback)?;
    let method = presign.method.as_deref().unwrap_or("PUT");
    ensure!(
        method.eq_ignore_ascii_case("PUT"),
        "unsupported presigned upload method {method:?}"
    );
    let mut headers: BTreeMap<String, String> = presign
        .headers
        .into_iter()
        .filter(|(k, _)| !is_forbidden_relay_header(k))
        .collect();
    headers
        .entry("Content-Type".to_string())
        .or_insert_with(|| "application/gzip".to_string());
    let (status, resp_body) = http_request(
        "PUT",
        &upload_url,
        &headers,
        None,
        Some(&archive_bytes),
        64 * 1024,
    )?;
    ensure!(
        (200..300).contains(&status),
        "cache upload failed with HTTP {status}: {}",
        sanitize_error_snippet(&resp_body)
    );

    let _ = write_local_cache_state(
        root,
        &LocalCacheState {
            last_digest: archive_sha256.clone(),
            schema_version: manifest.schema_version,
            repo_root_hash: repo_root_hash.to_string(),
        },
    );

    Ok(PushReport {
        scope,
        space_id: space_id.map(str::to_string),
        repo_root_hash: repo_root_hash.to_string(),
        schema_version: manifest.schema_version,
        digest: archive_sha256,
        bytes: archive_bytes.len(),
        entries: manifest.entries.len(),
    })
}

/// Request a presigned download URL from GitPulse Relay, download, verify
/// SHA-256, and atomically unpack into `db_path`.
/// Test-only shorthand; the `cache pull` command calls [`pull_cache_opts`].
#[cfg(test)]
pub fn pull_cache(
    root: &Path,
    db_path: &Path,
    relay_url: &str,
    space_id: Option<&str>,
    expected_digest: Option<&str>,
    repo_root_hash: &str,
    bearer_token: &str,
) -> Result<(CacheManifest, String)> {
    pull_cache_opts(
        root,
        db_path,
        relay_url,
        space_id,
        expected_digest,
        repo_root_hash,
        bearer_token,
        &UnpackOptions {
            expected_repo_hash: Some(repo_root_hash.to_string()),
            allow_foreign_repo: false,
            require_git_head: false,
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub fn pull_cache_opts(
    root: &Path,
    db_path: &Path,
    relay_url: &str,
    space_id: Option<&str>,
    expected_digest: Option<&str>,
    repo_root_hash: &str,
    bearer_token: &str,
    unpack_opts: &UnpackOptions,
) -> Result<(CacheManifest, String)> {
    let token = validate_relay_token(bearer_token)?;
    let base = validate_relay_or_presign_url(relay_url, "--relay-url")?;
    let relay_is_loopback = base.starts_with("http://");
    let requested_digest = match expected_digest {
        Some(d) => validate_hex64(d, "--digest")?,
        None => read_local_cache_state(root)
            .filter(|s| {
                s.schema_version == CURRENT_SCHEMA_VERSION && s.repo_root_hash == repo_root_hash
            })
            .map(|s| s.last_digest)
            .ok_or_else(|| {
                anyhow!(
                    "--digest <64-hex> is required for `devmap cache pull` when no prior pushed digest is recorded locally"
                )
            })?,
    };
    let scope = if space_id.is_some() {
        "space".to_string()
    } else {
        "account".to_string()
    };
    let req = PresignRequest {
        scope,
        space_id: space_id.map(str::to_string),
        repo_hash: repo_root_hash.to_string(),
        schema_version: CURRENT_SCHEMA_VERSION,
        digest: Some(requested_digest.clone()),
        action: "download".to_string(),
        bytes: None,
    };
    let presign_url = format!("{base}/v1/devmap-cache/presign");
    let presign = request_presign(&presign_url, &token, &req)?;
    // C7: presigned URL MUST use https:// unless --relay-url itself is loopback http://.
    let download_url =
        validate_url_with_policy(&presign.url, "presigned download URL", relay_is_loopback)?;
    let method = presign.method.as_deref().unwrap_or("GET");
    ensure!(
        method.eq_ignore_ascii_case("GET"),
        "unsupported presigned download method {method:?}"
    );

    if let Some(srv_d) = presign.digest.or(presign.sha256) {
        let srv_d = validate_hex64(&srv_d, "presign response digest")?;
        ensure!(
            requested_digest == srv_d,
            "presign response digest {srv_d} does not match requested digest {requested_digest}"
        );
    }

    let safe_headers: BTreeMap<String, String> = presign
        .headers
        .into_iter()
        .filter(|(k, _)| !is_forbidden_relay_header(k))
        .collect();
    let (status, archive_bytes) = http_request(
        "GET",
        &download_url,
        &safe_headers,
        None,
        None,
        MAX_ARCHIVE_BYTES,
    )?;
    ensure!(
        (200..300).contains(&status),
        "cache download failed with HTTP {status}: {}",
        sanitize_error_snippet(&archive_bytes)
    );
    let actual_digest = sha256_hex(&archive_bytes);
    ensure!(
        actual_digest == requested_digest,
        "downloaded cache SHA-256 mismatch: expected {requested_digest}, got {actual_digest}"
    );

    let manifest = unpack_bytes_into_store_opts(root, db_path, &archive_bytes, unpack_opts)?;
    let _ = write_local_cache_state(
        root,
        &LocalCacheState {
            last_digest: actual_digest.clone(),
            schema_version: manifest.schema_version,
            repo_root_hash: repo_root_hash.to_string(),
        },
    );
    Ok((manifest, actual_digest))
}

fn read_local_cache_state(root: &Path) -> Option<LocalCacheState> {
    let p = state_dir(root).join(CACHE_STATE_FILE);
    let bytes = std::fs::read(p).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(LocalCacheState {
        last_digest: v.get("last_digest")?.as_str()?.to_string(),
        schema_version: v.get("schema_version")?.as_i64()? as i32,
        repo_root_hash: v.get("repo_root_hash")?.as_str()?.to_string(),
    })
}

fn write_local_cache_state(root: &Path, state: &LocalCacheState) -> Result<()> {
    let p = state_dir(root).join(CACHE_STATE_FILE);
    ensure_parent(&p)?;
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "last_digest": state.last_digest,
        "schema_version": state.schema_version,
        "repo_root_hash": state.repo_root_hash,
    }))?;
    write_file_atomic(&p, &bytes)
}

fn request_presign(url: &str, bearer_token: &str, req: &PresignRequest) -> Result<PresignResponse> {
    let body = req.to_json_bytes()?;
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());
    let (status, resp_bytes) = http_request(
        "POST",
        url,
        &headers,
        Some(bearer_token),
        Some(&body),
        1024 * 1024,
    )?;
    ensure!(
        (200..300).contains(&status),
        "relay presign request failed with HTTP {status}: {}",
        sanitize_error_snippet(&resp_bytes)
    );
    PresignResponse::from_json_slice(&resp_bytes)
}

fn sanitize_error_snippet(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    for ch in raw.chars().take(160) {
        if ch.is_control() {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Minimal zero-dependency HTTP/1.1 (loopback) + curl-config (HTTPS) transport
// ---------------------------------------------------------------------------

fn http_request(
    method: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    bearer_token: Option<&str>,
    body: Option<&[u8]>,
    max_response_bytes: usize,
) -> Result<(u16, Vec<u8>)> {
    if url.starts_with("http://") {
        http_loopback_request(method, url, headers, bearer_token, body, max_response_bytes)
    } else if url.starts_with("https://") {
        https_curl_request(method, url, headers, bearer_token, body, max_response_bytes)
    } else {
        bail!("unsupported URL scheme in {url:?}")
    }
}

fn http_loopback_request(
    method: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    bearer_token: Option<&str>,
    body: Option<&[u8]>,
    max_response_bytes: usize,
) -> Result<(u16, Vec<u8>)> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("expected http:// URL"))?;
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let connect_addr = if authority.contains(':') && !authority.ends_with(']') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let mut stream = TcpStream::connect(&connect_addr)
        .with_context(|| format!("failed to connect to {connect_addr}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    let payload = body.unwrap_or(&[]);
    let mut req = format!(
        "{method} {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nContent-Length: {}\r\n",
        payload.len()
    );
    if let Some(tok) = bearer_token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    for (k, v) in headers {
        ensure!(
            !k.bytes().any(|b| b == b'\r' || b == b'\n')
                && !v.bytes().any(|b| b == b'\r' || b == b'\n'),
            "invalid HTTP header"
        );
        if !is_forbidden_relay_header(k) {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes())?;
    if !payload.is_empty() {
        stream.write_all(payload)?;
    }
    stream.flush()?;

    // Read response with hard upper bound (headers + max_response_bytes).
    let max_wire = max_response_bytes.saturating_add(64 * 1024);
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ensure!(
            raw.len().saturating_add(n) <= max_wire,
            "HTTP response exceeds maximum allowed size of {max_response_bytes} bytes"
        );
        raw.extend_from_slice(&buf[..n]);
    }

    let hdr_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response (missing header terminator)"))?;
    let hdr_text = std::str::from_utf8(&raw[..hdr_end])
        .map_err(|_| anyhow!("non-UTF-8 HTTP response headers"))?;
    let mut lines = hdr_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| anyhow!("empty HTTP status line"))?;
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("malformed HTTP status line {status_line:?}"))?
        .parse()
        .context("invalid HTTP status code")?;

    let mut is_chunked = false;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.to_ascii_lowercase().contains("chunked")
            {
                is_chunked = true;
            }
        }
    }

    let body_raw = &raw[hdr_end + 4..];
    let decoded = if is_chunked {
        decode_chunked(body_raw, max_response_bytes)?
    } else {
        ensure!(
            body_raw.len() <= max_response_bytes,
            "HTTP response body ({} bytes) exceeds maximum allowed size ({max_response_bytes} bytes)",
            body_raw.len()
        );
        body_raw.to_vec()
    };
    Ok((status_code, decoded))
}

fn decode_chunked(raw: &[u8], max_bytes: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        let nl = raw[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow!("truncated chunked response"))?;
        let size_line = std::str::from_utf8(&raw[pos..pos + nl])
            .map_err(|_| anyhow!("non-UTF-8 chunk size"))?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_line, 16)
            .with_context(|| format!("invalid chunk size {size_line:?}"))?;
        pos += nl + 2;
        if size == 0 {
            break;
        }
        ensure!(
            out.len().saturating_add(size) <= max_bytes,
            "chunked HTTP response exceeds maximum allowed size ({max_bytes} bytes)"
        );
        ensure!(
            pos.checked_add(size + 2).is_some_and(|e| e <= raw.len()),
            "truncated chunk payload"
        );
        out.extend_from_slice(&raw[pos..pos + size]);
        pos += size + 2;
    }
    Ok(out)
}

fn resolve_trusted_curl_binary() -> Result<PathBuf> {
    #[cfg(unix)]
    const CANDIDATES: &[&str] = &["/usr/bin/curl", "/bin/curl"];
    #[cfg(not(unix))]
    const CANDIDATES: &[&str] = &["C:\\Windows\\System32\\curl.exe"];

    for candidate in CANDIDATES {
        let p = Path::new(candidate);
        if let Ok(meta) = std::fs::metadata(p) {
            if !meta.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.uid() != 0 {
                    continue;
                }
            }
            return Ok(p.to_path_buf());
        }
    }
    bail!("trusted root-owned system curl binary not found in /usr/bin or /bin")
}

fn build_hardened_curl_args(
    method: &str,
    max_response_bytes: usize,
    cfg_path: &Path,
    resp_path: &Path,
    body_path: Option<&Path>,
    url: &str,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        // C6: `-q` MUST be the very first argument so ~/.curlrc is never read.
        "-q".into(),
        "--globoff".into(),
        "--proto".into(),
        "=https".into(),
        "--proto-redir".into(),
        "=https".into(),
        "--tlsv1.2".into(),
        "--max-redirs".into(),
        "0".into(),
        "--silent".into(),
        "--show-error".into(),
        "--max-time".into(),
        "60".into(),
        "--max-filesize".into(),
        max_response_bytes.to_string().into(),
        "-X".into(),
        method.into(),
        "--config".into(),
        cfg_path.as_os_str().to_os_string(),
        "-o".into(),
        resp_path.as_os_str().to_os_string(),
        "-w".into(),
        "%{http_code}".into(),
    ];
    if let Some(bp) = body_path {
        args.push("--data-binary".into());
        args.push(format!("@{}", bp.display()).into());
    }
    args.push(url.into());
    args
}

fn create_private_temp_dir() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mix =
        sha256_hex(format!("{}-{nanos}-{}", std::process::id(), next_unique_suffix()).as_bytes());
    let tmp_dir = std::env::temp_dir().join(format!("devmap-cache-{}", &mix[..24]));
    // Only Unix has a mode to set.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&tmp_dir)
        .with_context(|| format!("cannot create private temp dir {}", tmp_dir.display()))?;
    Ok(tmp_dir)
}

/// HTTPS transport via `curl` with `-q` (no ~/.curlrc), `--globoff`,
/// `--max-redirs 0`, `--proto =https`, trusted absolute curl path, and
/// credentials passed strictly in a mode-0600 `--config` file (never in argv).
fn https_curl_request(
    method: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    bearer_token: Option<&str>,
    body: Option<&[u8]>,
    max_response_bytes: usize,
) -> Result<(u16, Vec<u8>)> {
    let curl_bin = resolve_trusted_curl_binary()?;
    let tmp_dir = create_private_temp_dir()?;
    let _cleanup = DirCleanupGuard(tmp_dir.clone());

    let cfg_path = tmp_dir.join("curl.cfg");
    let resp_path = tmp_dir.join("resp.bin");
    let body_path = tmp_dir.join("req.bin");

    let mut cfg_contents = String::new();
    if let Some(tok) = bearer_token {
        let safe_tok = validate_relay_token(tok)?;
        cfg_contents.push_str(&format!("header = \"Authorization: Bearer {safe_tok}\"\n"));
    }
    for (k, v) in headers {
        if is_forbidden_relay_header(k) {
            continue;
        }
        ensure!(
            !k.bytes()
                .any(|b| b == b'\r' || b == b'\n' || b == b'"' || b == b'\\')
                && !v
                    .bytes()
                    .any(|b| b == b'\r' || b == b'\n' || b == b'"' || b == b'\\'),
            "unsafe character in HTTP header"
        );
        cfg_contents.push_str(&format!("header = \"{k}: {v}\"\n"));
    }
    write_private_file(&cfg_path, cfg_contents.as_bytes())?;

    let body_arg = if let Some(payload) = body {
        write_private_file(&body_path, payload)?;
        Some(body_path.as_path())
    } else {
        None
    };

    let args = build_hardened_curl_args(
        method,
        max_response_bytes,
        &cfg_path,
        &resp_path,
        body_arg,
        url,
    );
    let output = std::process::Command::new(&curl_bin)
        .args(&args)
        .output()
        .with_context(|| {
            format!(
                "failed to execute {} for HTTPS cache transfer",
                curl_bin.display()
            )
        })?;
    ensure!(
        output.status.success(),
        "HTTPS transfer failed: {}",
        sanitize_error_snippet(&output.stderr)
    );
    let status_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let status: u16 = status_text
        .parse()
        .with_context(|| format!("unexpected curl HTTP status output {status_text:?}"))?;
    let resp_meta = std::fs::metadata(&resp_path)?;
    ensure!(
        resp_meta.len() <= max_response_bytes as u64,
        "HTTPS response ({} bytes) exceeds maximum allowed size ({max_response_bytes} bytes)",
        resp_meta.len()
    );
    let resp_bytes = std::fs::read(&resp_path)?;
    Ok((status, resp_bytes))
}

fn write_private_file(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache_archive::{gzip_compress, write_tar};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devmap-cache-test-{label}-{}",
            next_unique_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_indexed_repo(label: &str) -> (PathBuf, PathBuf) {
        let repo = scratch_dir(label);
        std::fs::write(
            repo.join("lib.rs"),
            "pub fn cache_roundtrip_fn() -> u32 { 42 }\n",
        )
        .unwrap();
        let db = devmap_extract::paths::store_path(&repo);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let store = Store::open(&db).unwrap();
        let extraction =
            devmap_extract::extract_file("lib.rs", "pub fn cache_roundtrip_fn() -> u32 { 42 }\n");
        let resolved = devmap_resolve::Resolver::new()
            .resolve_all(std::slice::from_ref(&extraction))
            .unwrap();
        let analysis = devmap_analyze::analyze(std::slice::from_ref(&extraction), &resolved);
        store
            .save_generation(&[extraction], &resolved, &analysis)
            .unwrap();
        (repo, db)
    }

    #[test]
    fn pack_and_unpack_roundtrip_restores_queryable_store() {
        let (src_repo, src_db) = build_indexed_repo("pack-src");
        let dst_repo = scratch_dir("pack-dst");
        let dst_db = devmap_extract::paths::store_path(&dst_repo);

        let repo_hash = resolve_repo_hash(&src_repo, None).unwrap();
        let (gz, manifest, digest) = pack_store_to_bytes(&src_repo, &src_db, &repo_hash).unwrap();
        assert_eq!(manifest.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(manifest.format_version, CACHE_FORMAT_VERSION);
        assert_eq!(digest.len(), 64);

        let unpacked = unpack_bytes_into_store(&dst_repo, &dst_db, &gz).unwrap();
        assert_eq!(unpacked.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(dst_db.is_file());

        let reopened = Store::open_existing(&dst_db).unwrap().unwrap();
        let engine = devmap_query::StoreQueryEngine::new(&reopened);
        let resp = engine
            .search(devmap_query::Request {
                query: "cache_roundtrip_fn".to_string(),
                token_budget: 2000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .unwrap();
        assert!(
            resp.items
                .iter()
                .any(|h| h.symbol_name.contains("cache_roundtrip_fn")),
            "unpacked store must serve indexed symbols: {:?}",
            resp.items
        );

        let _ = std::fs::remove_dir_all(&src_repo);
        let _ = std::fs::remove_dir_all(&dst_repo);
    }

    #[test]
    fn unpack_rejects_schema_mismatch_and_bad_entry_digest() {
        let (src_repo, src_db) = build_indexed_repo("bad-manifest");
        let dst_repo = scratch_dir("bad-manifest-dst");
        let dst_db = devmap_extract::paths::store_path(&dst_repo);
        let db_bytes = std::fs::read(&src_db).unwrap();

        // 1. Schema version mismatch
        let bad_schema = CacheManifest {
            format_version: CACHE_FORMAT_VERSION,
            schema_version: CURRENT_SCHEMA_VERSION - 1,
            extraction_schema_version: devmap_extract::cache::EXTRACTION_SCHEMA_VERSION.to_string(),
            repo_root_hash: sha256_hex(b"repo"),
            git_head: String::new(),
            entries: vec![CacheManifestEntry {
                path: DB_ENTRY_NAME.to_string(),
                size: db_bytes.len() as u64,
                sha256: sha256_hex(&db_bytes),
            }],
            manifest_hmac: None,
        };
        let gz = pack_tar_gz(&[
            (
                MANIFEST_ENTRY_NAME.to_string(),
                serde_json::to_vec(&bad_schema.to_json_value()).unwrap(),
            ),
            (DB_ENTRY_NAME.to_string(), db_bytes.clone()),
        ])
        .unwrap();
        let err = unpack_bytes_into_store(&dst_repo, &dst_db, &gz).unwrap_err();
        assert!(err.to_string().contains("schema_version"), "{err}");
        assert!(!dst_db.exists());

        // 2. Bad SHA-256 digest in manifest
        let bad_digest = CacheManifest {
            format_version: CACHE_FORMAT_VERSION,
            schema_version: CURRENT_SCHEMA_VERSION,
            extraction_schema_version: devmap_extract::cache::EXTRACTION_SCHEMA_VERSION.to_string(),
            repo_root_hash: sha256_hex(b"repo"),
            git_head: String::new(),
            entries: vec![CacheManifestEntry {
                path: DB_ENTRY_NAME.to_string(),
                size: db_bytes.len() as u64,
                sha256: "0".repeat(64),
            }],
            manifest_hmac: None,
        };
        let gz = pack_tar_gz(&[
            (
                MANIFEST_ENTRY_NAME.to_string(),
                serde_json::to_vec(&bad_digest.to_json_value()).unwrap(),
            ),
            (DB_ENTRY_NAME.to_string(), db_bytes),
        ])
        .unwrap();
        let err = unpack_bytes_into_store(&dst_repo, &dst_db, &gz).unwrap_err();
        assert!(err.to_string().contains("SHA-256 digest mismatch"), "{err}");
        assert!(!dst_db.exists());

        let _ = std::fs::remove_dir_all(&src_repo);
        let _ = std::fs::remove_dir_all(&dst_repo);
    }

    #[test]
    fn unpack_rejects_undeclared_extra_file() {
        let (src_repo, src_db) = build_indexed_repo("extra-file");
        let dst_repo = scratch_dir("extra-file-dst");
        let dst_db = devmap_extract::paths::store_path(&dst_repo);
        let db_bytes = std::fs::read(&src_db).unwrap();

        let manifest = CacheManifest {
            format_version: CACHE_FORMAT_VERSION,
            schema_version: CURRENT_SCHEMA_VERSION,
            extraction_schema_version: devmap_extract::cache::EXTRACTION_SCHEMA_VERSION.to_string(),
            repo_root_hash: sha256_hex(b"repo"),
            git_head: String::new(),
            entries: vec![CacheManifestEntry {
                path: DB_ENTRY_NAME.to_string(),
                size: db_bytes.len() as u64,
                sha256: sha256_hex(&db_bytes),
            }],
            manifest_hmac: None,
        };
        let tar = write_tar(&[
            (
                MANIFEST_ENTRY_NAME.to_string(),
                serde_json::to_vec(&manifest.to_json_value()).unwrap(),
            ),
            (DB_ENTRY_NAME.to_string(), db_bytes),
            ("sneaky.bin".to_string(), b"injected".to_vec()),
        ])
        .unwrap();
        let gz = gzip_compress(&tar);
        let err = unpack_bytes_into_store(&dst_repo, &dst_db, &gz).unwrap_err();
        assert!(err.to_string().contains("undeclared extra file"), "{err}");

        let _ = std::fs::remove_dir_all(&src_repo);
        let _ = std::fs::remove_dir_all(&dst_repo);
    }

    #[test]
    fn push_and_pull_mock_http_presign_server() {
        let (src_repo, src_db) = build_indexed_repo("http-src");
        let dst_repo = scratch_dir("http-dst");
        let dst_db = devmap_extract::paths::store_path(&dst_repo);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let blobs: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::new(Mutex::new(BTreeMap::new()));
        let tamper = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_blobs = Arc::clone(&blobs);
        let server_tamper = Arc::clone(&tamper);
        let server_base = base_url.clone();

        let handle = std::thread::spawn(move || {
            // Serve 6 sequential requests: push(presign+PUT), pull(presign+GET), tampered pull(presign+GET)
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos);
                            let hdr = std::str::from_utf8(&raw[..pos]).unwrap();
                            for line in hdr.split("\r\n") {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(pos) = header_end {
                        if raw.len() >= pos + 4 + content_len {
                            break;
                        }
                    }
                }
                let pos = header_end.unwrap();
                let hdr = std::str::from_utf8(&raw[..pos]).unwrap();
                let body = &raw[pos + 4..pos + 4 + content_len];
                let first_line = hdr.split("\r\n").next().unwrap();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap();
                let path = parts.next().unwrap();

                let (status, resp_body) = if method == "POST" && path == "/v1/devmap-cache/presign"
                {
                    let has_auth = hdr
                        .split("\r\n")
                        .any(|l| l.eq_ignore_ascii_case("Authorization: Bearer secret-relay-tok"));
                    if !has_auth {
                        (401, br#"{"error":"unauthorized"}"#.to_vec())
                    } else {
                        let req: serde_json::Value = serde_json::from_slice(body).unwrap();
                        let action = req["action"].as_str().unwrap();
                        let digest = req["digest"].as_str().unwrap_or("latest");
                        let obj_url = format!("{server_base}/r2/{digest}");
                        let resp = serde_json::json!({
                            "url": obj_url,
                            "method": if action == "upload" { "PUT" } else { "GET" },
                            "digest": digest,
                            "headers": {
                                "X-Amz-Checksum-Sha256": "dGVzdC1jaGVja3N1bS1iNjQ=",
                                "X-Amz-Content-Sha256": digest,
                                "Authorization": "Bearer should-be-stripped",
                            },
                        });
                        (200, serde_json::to_vec(&resp).unwrap())
                    }
                } else if method == "PUT" && path.starts_with("/r2/") {
                    let key = path.trim_start_matches("/r2/").to_string();
                    // Opus #85 residual (1): verify x-amz-checksum-sha256 & x-amz-content-sha256
                    // are forwarded verbatim while forbidden Authorization is stripped.
                    let has_checksum = hdr.split("\r\n").any(|l| {
                        l.eq_ignore_ascii_case("X-Amz-Checksum-Sha256: dGVzdC1jaGVja3N1bS1iNjQ=")
                    });
                    let has_content_sha = hdr
                        .split("\r\n")
                        .any(|l| l.eq_ignore_ascii_case(&format!("X-Amz-Content-Sha256: {key}")));
                    let leaked_auth = hdr
                        .split("\r\n")
                        .any(|l| l.to_ascii_lowercase().starts_with("authorization:"));
                    if !has_checksum || !has_content_sha || leaked_auth {
                        (403, b"sigv4 header mismatch".to_vec())
                    } else {
                        server_blobs.lock().unwrap().insert(key, body.to_vec());
                        (200, b"ok".to_vec())
                    }
                } else if method == "GET" && path.starts_with("/r2/") {
                    let key = path.trim_start_matches("/r2/").to_string();
                    let mut data = server_blobs.lock().unwrap().get(&key).cloned().unwrap();
                    if server_tamper.load(Ordering::SeqCst) {
                        data[0] ^= 0xFF;
                    }
                    (200, data)
                } else {
                    (404, b"not found".to_vec())
                };

                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    resp_body.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
                stream.write_all(&resp_body).unwrap();
                stream.flush().unwrap();
            }
        });

        let repo_hash = resolve_repo_hash(&src_repo, None).unwrap();
        let pushed = push_cache(
            &src_repo,
            &src_db,
            &base_url,
            Some("space-123"),
            &repo_hash,
            "secret-relay-tok",
        )
        .unwrap();
        assert_eq!(pushed.scope, "space");
        assert_eq!(pushed.schema_version, CURRENT_SCHEMA_VERSION);

        let (pulled_manifest, pulled_digest) = pull_cache(
            &dst_repo,
            &dst_db,
            &base_url,
            Some("space-123"),
            Some(&pushed.digest),
            &repo_hash,
            "secret-relay-tok",
        )
        .unwrap();
        assert_eq!(pulled_digest, pushed.digest);
        assert_eq!(pulled_manifest.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(dst_db.is_file());

        // Tampered download must fail closed on SHA-256 verification.
        tamper.store(true, Ordering::SeqCst);
        let dst_repo2 = scratch_dir("http-dst-tampered");
        let dst_db2 = devmap_extract::paths::store_path(&dst_repo2);
        let err = pull_cache(
            &dst_repo2,
            &dst_db2,
            &base_url,
            Some("space-123"),
            Some(&pushed.digest),
            &repo_hash,
            "secret-relay-tok",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("SHA-256 mismatch"),
            "tampered download must fail SHA-256 check: {err}"
        );
        assert!(!dst_db2.exists());

        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&src_repo);
        let _ = std::fs::remove_dir_all(&dst_repo);
        let _ = std::fs::remove_dir_all(&dst_repo2);
    }

    #[test]
    fn non_loopback_http_relay_url_and_ssrf_presign_url_are_rejected() {
        let err =
            validate_relay_or_presign_url("http://relay.example.com", "--relay-url").unwrap_err();
        assert!(err.to_string().contains("https://"), "{err}");
        assert!(validate_relay_or_presign_url("https://relay.example.com/", "--relay-url").is_ok());
        assert!(validate_relay_or_presign_url("http://127.0.0.1:8787", "--relay-url").is_ok());
        // Reject userinfo trick (127.0.0.1:80@evil.com)
        assert!(
            validate_relay_or_presign_url("http://127.0.0.1:80@evil.com/x", "--relay-url").is_err()
        );
        // C7: HTTPS relay must reject loopback HTTP presigned URL (SSRF prevention)
        assert!(
            validate_url_with_policy("http://127.0.0.1:9000/internal", "presign", false).is_err()
        );
        assert!(validate_url_with_policy("https://r2.example.com/obj", "presign", false).is_ok());
        // C6: curl globbing braces rejected in URL, quotes/backslashes rejected in token
        assert!(
            validate_url_with_policy("https://r2.example.com/{a,b}", "presign", false).is_err()
        );
        assert!(validate_relay_token("good-token_123").is_ok());
        assert!(validate_relay_token("bad\"token").is_err());
        assert!(validate_relay_token("bad\\token").is_err());
        // C6: `-q` is the very first curl argument and `--globoff` + `--max-redirs 0` are present
        let args = build_hardened_curl_args(
            "PUT",
            1024,
            Path::new("/tmp/c"),
            Path::new("/tmp/r"),
            None,
            "https://r2.example.com/obj",
        );
        assert_eq!(args[0], "-q");
        assert!(args.iter().any(|a| a == "--globoff"));
        assert!(args.iter().any(|a| a == "--max-redirs"));
        assert!(is_forbidden_relay_header("Host"));
        assert!(is_forbidden_relay_header("Authorization"));
    }

    #[test]
    fn unpack_rejects_foreign_repo_hash_unless_explicitly_allowed() {
        let (src_repo, src_db) = build_indexed_repo("foreign-src");
        let dst_repo = scratch_dir("foreign-dst");
        let dst_db = devmap_extract::paths::store_path(&dst_repo);

        let src_hash = resolve_repo_hash(&src_repo, None).unwrap();
        let dst_hash = resolve_repo_hash(&dst_repo, None).unwrap();
        assert_ne!(src_hash, dst_hash);

        let (gz, _, _) = pack_store_to_bytes(&src_repo, &src_db, &src_hash).unwrap();
        let err = unpack_bytes_into_store_opts(
            &dst_repo,
            &dst_db,
            &gz,
            &UnpackOptions {
                expected_repo_hash: Some(dst_hash.clone()),
                allow_foreign_repo: false,
                require_git_head: false,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("repo_root_hash"), "{err}");
        assert!(!dst_db.exists());

        // Succeeds when explicitly opted in with allow_foreign_repo: true
        unpack_bytes_into_store_opts(
            &dst_repo,
            &dst_db,
            &gz,
            &UnpackOptions {
                expected_repo_hash: Some(dst_hash),
                allow_foreign_repo: true,
                require_git_head: false,
            },
        )
        .unwrap();
        assert!(dst_db.is_file());

        let _ = std::fs::remove_dir_all(&src_repo);
        let _ = std::fs::remove_dir_all(&dst_repo);
    }

    #[test]
    fn pack_strips_planted_secret_literal_from_raw_sqlite_and_extractions() {
        const SECRET_MARKER: &str = "AKIA_TEST_PLANTED_LITERAL_9988776655";
        let repo = scratch_dir("secret-scrub");
        let src = format!(
            "/// Docstring with {SECRET_MARKER}\npub fn leak_probe() -> &'static str {{ \"{SECRET_MARKER}\" }}\n"
        );
        std::fs::write(repo.join("lib.rs"), &src).unwrap();
        let db = devmap_extract::paths::store_path(&repo);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        {
            let store = Store::open(&db).unwrap();
            let ext = devmap_extract::extract_file("lib.rs", &src);
            let res = devmap_resolve::Resolver::new()
                .resolve_all(std::slice::from_ref(&ext))
                .unwrap();
            let analysis = devmap_analyze::analyze(std::slice::from_ref(&ext), &res);
            store.save_generation(&[ext], &res, &analysis).unwrap();
        }

        let repo_hash = resolve_repo_hash(&repo, None).unwrap();
        let (gz, _, _) = pack_store_to_bytes(&repo, &db, &repo_hash).unwrap();
        let entries = unpack_tar_gz_strict(&gz, MAX_ARCHIVE_BYTES).unwrap();
        let (_, packed_db_bytes) = entries
            .iter()
            .find(|(p, _)| p == DB_ENTRY_NAME)
            .expect("packed archive must contain devmap.sqlite");

        // 1. Raw SQLite bytes (including freed pages) must contain ZERO occurrences of the secret.
        assert!(
            !packed_db_bytes
                .windows(SECRET_MARKER.len())
                .any(|w| w == SECRET_MARKER.as_bytes()),
            "planted secret literal survived in raw packed SQLite bytes!"
        );

        // 2. Decompressed extraction_json rows inside the unpacked DB must also contain ZERO occurrences.
        let dst_repo = scratch_dir("secret-scrub-dst");
        let dst_db = devmap_extract::paths::store_path(&dst_repo);
        unpack_bytes_into_store_opts(
            &dst_repo,
            &dst_db,
            &gz,
            &UnpackOptions {
                expected_repo_hash: Some(repo_hash),
                allow_foreign_repo: false,
                require_git_head: false,
            },
        )
        .unwrap();
        let reopened = Store::open_read_only(&dst_db).unwrap();
        for ext in reopened.latest_extractions().unwrap() {
            let serialized = serde_json::to_string(&ext).unwrap();
            assert!(
                !serialized.contains(SECRET_MARKER),
                "planted secret literal survived in stored extraction payload: {serialized}"
            );
        }
        // Symbol search still works after literal scrubbing.
        let engine = devmap_query::StoreQueryEngine::new(&reopened);
        let resp = engine
            .search(devmap_query::Request {
                query: "leak_probe".to_string(),
                token_budget: 2000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .unwrap();
        assert!(resp
            .items
            .iter()
            .any(|h| h.symbol_name.contains("leak_probe")));

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&dst_repo);
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_and_detects_manifest_tampering() {
        // RFC 4231 Test Case 2
        let mac = hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            mac,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert!(constant_time_eq_hex(&mac, &mac));
        assert!(!constant_time_eq_hex(&mac, &mac.to_uppercase()));
        assert!(!constant_time_eq_hex(
            &mac,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3844"
        ));

        let mut manifest = CacheManifest {
            format_version: CACHE_FORMAT_VERSION,
            schema_version: CURRENT_SCHEMA_VERSION,
            extraction_schema_version: devmap_extract::cache::EXTRACTION_SCHEMA_VERSION.to_string(),
            repo_root_hash: "a".repeat(64),
            git_head: "deadbeef".to_string(),
            entries: vec![CacheManifestEntry {
                path: DB_ENTRY_NAME.to_string(),
                size: 123,
                sha256: "b".repeat(64),
            }],
            manifest_hmac: None,
        };
        let key = b"test-signing-key";
        let unsigned = manifest.canonical_unsigned_bytes().unwrap();
        let sig = hmac_sha256_hex(key, &unsigned);
        manifest.manifest_hmac = Some(sig.clone());

        // Round-trip JSON preserves signature and verifies identically.
        let json_bytes = serde_json::to_vec(&manifest.to_json_value()).unwrap();
        let parsed = CacheManifest::from_json_slice(&json_bytes).unwrap();
        assert_eq!(parsed.manifest_hmac.as_deref(), Some(sig.as_str()));
        assert!(constant_time_eq_hex(
            &sig,
            &hmac_sha256_hex(key, &parsed.canonical_unsigned_bytes().unwrap())
        ));

        // Any manifest mutation invalidates the signature.
        let mut tampered = parsed;
        tampered.git_head = "cafebabe".to_string();
        let recomputed = hmac_sha256_hex(key, &tampered.canonical_unsigned_bytes().unwrap());
        assert!(!constant_time_eq_hex(&sig, &recomputed));
    }
}
