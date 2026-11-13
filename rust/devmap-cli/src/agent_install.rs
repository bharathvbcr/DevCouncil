//! Install Antigravity agent and rule files.
//!
//! This is the one writer of `.devcouncil-agents.json`. The receipt rules match
//! `skills` install: plan every path, then write; identical bytes are adopted;
//! a file whose bytes differ and whose receipt hash does not match the previous
//! contents is left alone; `--check` and `--dry-run` write nothing, including
//! the receipt. The document bound is the skill-library bound, not the 64 KiB
//! cap on `integrate --servers-stdin`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use serde_json::{json, Value};

use crate::skills::MAX_LIBRARY_BYTES;

const FILE_LIMIT: usize = 256 * 1024;
const RECEIPT_LIMIT: usize = 1024 * 1024;
const MAX_RECEIPT_ENTRIES: usize = 4096;
const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const RECEIPT_REL: &str = ".devcouncil-agents.json";
const LOCK_REL: &str = ".devcouncil-agents.lock";

/// Bound on an `--agents-stdin` document. Same ceiling as `--library-stdin`.
pub const MAX_DOCUMENT_BYTES: usize = MAX_LIBRARY_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFile {
    pub name: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDocument {
    pub agents: Vec<AgentFile>,
    pub rules: Vec<AgentFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInstallReport {
    /// Content paths that differ (check / dry-run) or that were written (apply).
    pub differing: Vec<PathBuf>,
    /// Paths written on a successful apply, including the receipt when it changed.
    pub written: Vec<PathBuf>,
    /// Every content path this document manages, whether or not it differs.
    pub managed: Vec<PathBuf>,
    pub receipt: PathBuf,
    pub check_ok: bool,
}

/// Parse `{"agents":[{"name","content"}],"rules":[{"name","content"}]}`.
///
/// Unknown keys are refused. Names and bodies are checked by the installer,
/// before anything is written.
pub fn parse_document(bytes: &[u8]) -> anyhow::Result<AgentDocument> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        bail!("agent document exceeds {MAX_DOCUMENT_BYTES} bytes");
    }
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|err| anyhow!("agent document is not JSON: {err}"))?;
    let object = document
        .as_object()
        .ok_or_else(|| anyhow!("agent document must be a JSON object"))?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "agents" | "rules"))
    {
        bail!("agent document: unknown key {key:?}");
    }
    if !object.contains_key("agents") || !object.contains_key("rules") {
        bail!("agent document needs `agents` and `rules` arrays");
    }
    Ok(AgentDocument {
        agents: parse_entries(object.get("agents"), "agent")?,
        rules: parse_entries(object.get("rules"), "rule")?,
    })
}

fn parse_entries(value: Option<&Value>, kind: &str) -> anyhow::Result<Vec<AgentFile>> {
    let entries = value
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("agent document needs `{kind}s` to be an array"))?;
    if entries.len() > MAX_ENTRIES {
        bail!("agent document: at most {MAX_ENTRIES} {kind}s");
    }
    entries
        .iter()
        .map(|entry| {
            let fields = entry
                .as_object()
                .ok_or_else(|| anyhow!("each {kind} must be a JSON object"))?;
            if let Some(key) = fields
                .keys()
                .find(|key| !matches!(key.as_str(), "name" | "content"))
            {
                bail!("{kind}: unknown key {key:?}");
            }
            let text = |key: &str| {
                fields
                    .get(key)
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("{kind} needs a string `{key}`"))
            };
            Ok(AgentFile {
                name: text("name")?.to_string(),
                content: text("content")?.as_bytes().to_vec(),
            })
        })
        .collect()
}

/// Install `agents` and `rules`, all or nothing.
///
/// Every name, path and body is checked, and every destination planned, before
/// the first write.
pub fn install_agents(
    project_root: &Path,
    agents: &[AgentFile],
    rules: &[AgentFile],
    dry_run: bool,
    check: bool,
) -> anyhow::Result<AgentInstallReport> {
    let root = project_root
        .canonicalize()
        .with_context(|| format!("agent destination root {}", project_root.display()))?;
    if !root.is_dir() {
        bail!(
            "{}: agent destination root is not a directory",
            root.display()
        );
    }
    if agents.len() + rules.len() > MAX_ENTRIES {
        bail!("agent install limit: at most {MAX_ENTRIES} files");
    }

    let mut rendered: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    place(&root, agents, true, &mut rendered)?;
    place(&root, rules, false, &mut rendered)?;
    let batch: usize = rendered.values().map(Vec::len).sum();
    if batch > MAX_BATCH_BYTES {
        bail!("agent install exceeds 8 MiB batch limit");
    }

    let receipt = validate_relative(&root, RECEIPT_REL)?;
    let managed: Vec<PathBuf> = rendered.keys().cloned().collect();
    let check_only = dry_run || check;
    let (plan, _hashes) = scaffold_plan(&root, &rendered, &receipt, check_only)?;
    let differing: Vec<PathBuf> = plan.iter().map(|step| step.target.clone()).collect();

    if check_only || rendered.is_empty() {
        return Ok(AgentInstallReport {
            check_ok: differing.is_empty(),
            differing,
            written: Vec::new(),
            managed,
            receipt,
        });
    }

    let lock = root.join(LOCK_REL);
    acquire_lock(&lock)?;
    let result = (|| -> anyhow::Result<AgentInstallReport> {
        let (plan, hashes) = scaffold_plan(&root, &rendered, &receipt, false)?;
        let mut written = Vec::with_capacity(plan.len());
        for step in &plan {
            validate_relative(&root, &step.relative)?;
            if read_bounded(&step.target, FILE_LIMIT)? != step.previous {
                bail!(
                    "{}: modified while installing agents",
                    step.target.display()
                );
            }
            if let Some(parent) = step.target.parent() {
                fs::create_dir_all(parent)?;
            }
            write_atomic_bytes(&step.target, &step.content)?;
            written.push(step.target.clone());
        }
        let document = json!({"schema": 1, "files": hashes});
        let encoded = serde_json::to_vec_pretty(&document)?;
        let prior = read_bounded(&receipt, RECEIPT_LIMIT)?;
        if prior.as_deref() != Some(encoded.as_slice()) {
            if let Some(parent) = receipt.parent() {
                fs::create_dir_all(parent)?;
            }
            write_atomic_bytes(&receipt, &encoded)?;
            written.push(receipt.clone());
        }
        Ok(AgentInstallReport {
            check_ok: true,
            differing: plan.iter().map(|step| step.target.clone()).collect(),
            written,
            managed,
            receipt,
        })
    })();
    let _ = fs::remove_dir(&lock);
    result
}

fn place(
    root: &Path,
    files: &[AgentFile],
    agent: bool,
    rendered: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> anyhow::Result<()> {
    let mut names = std::collections::BTreeSet::new();
    for file in files {
        validate_name(&file.name)?;
        if !names.insert(file.name.as_str()) {
            bail!("Conflicting duplicate agent name: {}", file.name);
        }
        if file.content.len() > FILE_LIMIT {
            bail!("{}: agent file exceeds byte limit", file.name);
        }
        validate_body(&file.name, &file.content, agent)?;
        let relative = if agent {
            format!(".agents/agents/{}/agent.md", file.name)
        } else {
            format!(".agents/rules/{}.md", file.name)
        };
        let target = validate_relative(root, &relative)?;
        rendered.insert(target, file.content.clone());
    }
    Ok(())
}

fn validate_body(name: &str, content: &[u8], agent: bool) -> anyhow::Result<()> {
    let text =
        std::str::from_utf8(content).map_err(|_| anyhow!("{name}: agent body is not UTF-8"))?;
    let fields = frontmatter(text, name)?;
    if agent {
        let declared = fields
            .get("name")
            .map(String::as_str)
            .ok_or_else(|| anyhow!("{name}: frontmatter must define name"))?;
        if unquote(declared) != name {
            bail!("{name}: frontmatter name {declared:?} != entry name");
        }
    } else {
        let trigger = fields
            .get("trigger")
            .map(String::as_str)
            .ok_or_else(|| anyhow!("{name}: rule frontmatter must set trigger: always_on"))?;
        if unquote(trigger) != "always_on" {
            bail!("{name}: rule trigger must be always_on, not {trigger:?}");
        }
    }
    Ok(())
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
}

fn frontmatter(text: &str, label: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let normalized = text.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") {
        bail!("{label}: must begin with '---' YAML frontmatter");
    }
    let rest = &normalized[4..];
    let end = rest
        .find("\n---\n")
        .ok_or_else(|| anyhow!("{label}: frontmatter is never closed by a '---' line"))?;
    let mut fields = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in rest[..end].split('\n') {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            let key = current
                .as_deref()
                .ok_or_else(|| anyhow!("{label}: frontmatter opens with a continuation line"))?;
            let joined = format!(
                "{} {}",
                fields.get(key).map(String::as_str).unwrap_or(""),
                line.trim()
            );
            fields.insert(key.to_string(), joined.trim().to_string());
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("{label}: frontmatter line is not 'key: value'"))?;
        let key = key.trim();
        if key.is_empty() || fields.contains_key(key) {
            bail!("{label}: duplicate or empty frontmatter key {key:?}");
        }
        fields.insert(key.to_string(), value.trim().to_string());
        current = Some(key.to_string());
    }
    Ok(fields)
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.chars().enumerate().all(|(i, c)| match (i, c) {
            (0, c) => c.is_ascii_lowercase() || c.is_ascii_digit(),
            (_, c) => c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-',
        });
    let reserved = matches!(
        name,
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    );
    if !ok || reserved {
        bail!("Invalid agent name: {name:?}");
    }
    Ok(())
}

fn validate_relative(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains(':')
        || relative.starts_with('/')
    {
        bail!("Invalid agent destination: {relative:?}");
    }
    let parts: Vec<&str> = relative.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        bail!("Invalid agent destination: {relative:?}");
    }
    let mut path = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        match path.symlink_metadata() {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    bail!("{}: refusing symlink in agent destination", path.display());
                }
                if index + 1 < parts.len() && !meta.is_dir() {
                    bail!("{}: agent parent is not a directory", path.display());
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(path)
}

struct Planned {
    target: PathBuf,
    relative: String,
    content: Vec<u8>,
    previous: Option<Vec<u8>>,
}

fn scaffold_plan(
    root: &Path,
    rendered: &BTreeMap<PathBuf, Vec<u8>>,
    receipt: &Path,
    check_only: bool,
) -> anyhow::Result<(Vec<Planned>, BTreeMap<String, String>)> {
    let _ = validate_relative(root, RECEIPT_REL)?;
    let saved = match read_bounded(receipt, RECEIPT_LIMIT)? {
        None => json!({"schema": 1, "files": {}}),
        Some(raw) => serde_json::from_slice(&raw)
            .map_err(|_| anyhow!("{}: invalid agent installation receipt", receipt.display()))?,
    };
    let schema = saved
        .get("schema")
        .and_then(Value::as_u64)
        .filter(|n| *n == 1);
    let files = saved.get("files").and_then(Value::as_object);
    if schema.is_none() || files.is_none() {
        bail!("{}: invalid agent installation receipt", receipt.display());
    }
    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in files.unwrap() {
        let digest = value
            .as_str()
            .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| {
                anyhow!(
                    "{}: invalid or oversized agent installation receipt",
                    receipt.display()
                )
            })?;
        hashes.insert(key.clone(), digest.to_ascii_lowercase());
    }
    if hashes.len() > MAX_RECEIPT_ENTRIES {
        bail!(
            "{}: agent installation receipt exceeds limit",
            receipt.display()
        );
    }

    let mut plan = Vec::new();
    for (target, content) in rendered {
        let key = target
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| target.display().to_string());
        let _ = validate_relative(root, &key)?;
        let current = read_bounded(target, FILE_LIMIT)?;
        if current.as_deref() != Some(content.as_slice()) {
            if !check_only {
                if let Some(existing) = &current {
                    let prior = hashes.get(&key).map(String::as_str);
                    let actual = sha256_hex(existing);
                    if prior != Some(actual.as_str()) {
                        bail!(
                            "{}: unmanaged or locally modified agent; preserve or move it before installing",
                            target.display()
                        );
                    }
                }
            }
            plan.push(Planned {
                target: target.clone(),
                relative: key.clone(),
                content: content.clone(),
                previous: current,
            });
        }
        hashes.insert(key, sha256_hex(content));
    }
    let encoded = serde_json::to_vec(&json!({"schema": 1, "files": hashes}))?;
    if hashes.len() > MAX_RECEIPT_ENTRIES || encoded.len() > RECEIPT_LIMIT {
        bail!(
            "{}: agent installation receipt exceeds limit",
            receipt.display()
        );
    }
    Ok((plan, hashes))
}

fn lock_is_contended(err: &std::io::Error, windows: bool) -> bool {
    match err.kind() {
        std::io::ErrorKind::AlreadyExists => true,
        std::io::ErrorKind::PermissionDenied => windows,
        _ => false,
    }
}

fn acquire_lock(lock: &Path) -> anyhow::Result<()> {
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match fs::create_dir(lock) {
            Ok(()) => return Ok(()),
            Err(err) if lock_is_contended(&err, cfg!(windows)) => {
                if Instant::now() >= deadline {
                    if err.kind() == std::io::ErrorKind::AlreadyExists {
                        bail!("Agent installer is busy: {}", lock.display());
                    }
                    return Err(err.into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn read_bounded(path: &Path, limit: usize) -> anyhow::Result<Option<Vec<u8>>> {
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        bail!(
            "{}: not a regular file within the {limit} byte limit",
            path.display()
        );
    }
    if meta.len() as usize > limit {
        bail!(
            "{}: not a regular file within the {limit} byte limit",
            path.display()
        );
    }
    let file = fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take((limit as u64) + 1).read_to_end(&mut buf)?;
    if buf.len() > limit {
        bail!("{}: file grew beyond byte limit", path.display());
    }
    Ok(Some(buf))
}

fn write_atomic_bytes(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        "{}.{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("agent"),
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn sha256_hex(data: &[u8]) -> String {
    crate::sha256::sha256_hex(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devmap-agents-{label}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn agent(name: &str, body: &str) -> AgentFile {
        AgentFile {
            name: name.to_string(),
            content: format!("---\nname: {name}\ndescription: d\n---\n{body}\n").into_bytes(),
        }
    }

    fn rule(name: &str) -> AgentFile {
        AgentFile {
            name: name.to_string(),
            content: format!("---\ndescription: d\ntrigger: always_on\n---\n# {name}\n")
                .into_bytes(),
        }
    }

    fn five() -> (Vec<AgentFile>, Vec<AgentFile>) {
        (
            [
                "devcouncil",
                "devcouncil-implementer",
                "devcouncil-reviewer",
                "devcouncil-verifier",
            ]
            .into_iter()
            .map(|name| agent(name, name))
            .collect(),
            vec![rule("devcouncil")],
        )
    }

    fn rel(root: &Path, path: &Path) -> String {
        path.strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    }

    #[test]
    fn the_document_bound_matches_the_skill_library_and_not_the_server_cap() {
        assert_eq!(MAX_DOCUMENT_BYTES, MAX_LIBRARY_BYTES);
        const { assert!(MAX_DOCUMENT_BYTES > 64 * 1024) };
        let mut oversized = vec![b' '; MAX_DOCUMENT_BYTES + 1];
        oversized[0] = b'{';
        let err = parse_document(&oversized).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn unknown_keys_are_refused() {
        let err = parse_document(br#"{"agents":[],"rules":[],"tools":[]}"#).unwrap_err();
        assert!(err.to_string().contains("unknown key"), "{err}");
        let err =
            parse_document(br#"{"agents":[{"name":"a","content":"x","tools":"Read"}],"rules":[]}"#)
                .unwrap_err();
        assert!(err.to_string().contains("unknown key"), "{err}");
    }

    #[test]
    fn install_writes_the_five_paths_and_the_receipt() {
        let root = scratch("five");
        let (agents, rules) = five();
        let report = install_agents(&root, &agents, &rules, false, false).unwrap();
        assert!(report.check_ok);
        let wrote: Vec<String> = report.written.iter().map(|p| rel(&root, p)).collect();
        for rel_path in [
            ".agents/agents/devcouncil/agent.md",
            ".agents/agents/devcouncil-implementer/agent.md",
            ".agents/agents/devcouncil-reviewer/agent.md",
            ".agents/agents/devcouncil-verifier/agent.md",
            ".agents/rules/devcouncil.md",
            RECEIPT_REL,
        ] {
            assert!(root.join(rel_path).is_file(), "{rel_path}");
            assert!(
                wrote.iter().any(|p| p == rel_path),
                "{wrote:?} missing {rel_path}"
            );
        }
        let again = install_agents(&root, &agents, &rules, false, false).unwrap();
        assert!(again.written.is_empty(), "identical bytes are current");
        assert!(again.check_ok);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn matching_bytes_with_no_receipt_write_only_the_receipt() {
        let root = scratch("adopt");
        let files = [agent("devcouncil", "body")];
        install_agents(&root, &files, &[], false, false).unwrap();
        fs::remove_file(root.join(RECEIPT_REL)).unwrap();
        let check = install_agents(&root, &files, &[], false, true).unwrap();
        assert!(
            check.check_ok,
            "a missing receipt beside matching bytes is current"
        );
        assert!(check.written.is_empty());
        assert!(
            !root.join(RECEIPT_REL).exists(),
            "--check wrote the receipt"
        );
        let again = install_agents(&root, &files, &[], false, false).unwrap();
        let wrote: Vec<String> = again.written.iter().map(|p| rel(&root, p)).collect();
        assert_eq!(wrote, vec![RECEIPT_REL.to_string()]);
        assert!(again.check_ok);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn check_reports_a_missing_file_as_drift_and_writes_nothing() {
        let root = scratch("check");
        let files = [agent("devcouncil", "body")];
        let report = install_agents(&root, &files, &[], false, true).unwrap();
        assert!(!report.check_ok);
        assert_eq!(report.differing.len(), 1);
        assert!(report.written.is_empty());
        assert!(!root.join(".agents").exists());
        assert!(!root.join(RECEIPT_REL).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let root = scratch("dry");
        let (agents, rules) = five();
        let report = install_agents(&root, &agents, &rules, true, false).unwrap();
        assert!(!report.check_ok);
        assert_eq!(report.differing.len(), 5);
        assert!(report.written.is_empty());
        assert!(report.managed.len() == 5);
        let entries = fs::read_dir(&root).unwrap().count();
        assert_eq!(entries, 0, "dry-run created a file");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symlink_is_refused_and_nothing_is_written() {
        let base = scratch("symlink");
        let outside = base.join("outside");
        let root = base.join("repo");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".agents")).unwrap();
        let err =
            install_agents(&root, &[agent("devcouncil", "x")], &[], false, false).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        assert_eq!(
            fs::read_dir(&outside).unwrap().count(),
            0,
            "followed the link"
        );
        assert!(!root.join(RECEIPT_REL).exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn an_unmanaged_local_edit_is_left_in_place() {
        let root = scratch("unmanaged");
        let files = [agent("devcouncil", "original")];
        install_agents(&root, &files, &[], false, false).unwrap();
        let path = root.join(".agents/agents/devcouncil/agent.md");
        let receipt = fs::read(root.join(RECEIPT_REL)).unwrap();
        fs::write(&path, "locally edited\n").unwrap();
        let err = install_agents(&root, &files, &[], false, false).unwrap_err();
        assert!(err.to_string().contains("unmanaged"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "locally edited\n");
        assert_eq!(fs::read(root.join(RECEIPT_REL)).unwrap(), receipt);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_entry_writes_nothing() {
        let root = scratch("bad");
        let err = install_agents(
            &root,
            &[
                agent("devcouncil", "ok"),
                AgentFile {
                    name: "..".to_string(),
                    content: b"---\nname: ..\n---\nbody\n".to_vec(),
                },
            ],
            &[],
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid agent name"), "{err}");
        assert!(!root.join(".agents").exists());
        assert!(!root.join(RECEIPT_REL).exists());
        let err = install_agents(
            &root,
            &[AgentFile {
                name: "devcouncil".to_string(),
                content: b"---\nname: other\n---\nbody\n".to_vec(),
            }],
            &[],
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("entry name"), "{err}");
        assert!(!root.join(".agents").exists());

        let err = install_agents(
            &root,
            &[AgentFile {
                name: "..".to_string(),
                content: b"---\nname: ..\n---\nbody\n".to_vec(),
            }],
            &[],
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid agent name"), "{err}");
        assert!(!root.join(RECEIPT_REL).exists());

        let err = install_agents(
            &root,
            &[],
            &[AgentFile {
                name: "devcouncil".to_string(),
                content: b"---\ndescription: d\ntrigger: alwaysOn\n---\nbody\n".to_vec(),
            }],
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("always_on"), "{err}");
        assert!(!root.join(".agents").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_does_not_create_a_state_marker() {
        let root = scratch("marker");
        install_agents(&root, &[agent("devcouncil", "x")], &[], false, false).unwrap();
        assert!(!root.join(".devcouncil").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
