//! Install the five embedded DevMap skills into host skill directories.
//!
//! Port of the DevMap-only path of `src/devcouncil/skills/registry.py`
//! `scaffold_skills`: receipt, unowned-file refusal, byte bounds, lock timeout.
//! Domain skills stay with DevCouncil's installer until Phase 4.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use serde_json::{json, Value};

use crate::claude::PLUGIN_SKILLS;

const SKILL_FILE_LIMIT: usize = 256 * 1024;
const SKILL_RECEIPT_LIMIT: usize = 1024 * 1024;
const MAX_RECEIPT_ENTRIES: usize = 4096;
const MAX_DESTINATIONS: usize = 16;
const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const RECEIPT_REL: &str = ".devcouncil-skills.json";
const LOCK_REL: &str = ".devcouncil-skills.lock";

/// Default destinations when the caller names none.
pub const DEFAULT_DESTINATIONS: &[&str] = &[".claude/skills", ".cursor/skills", ".agents/skills"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallPlan {
    pub target: PathBuf,
    pub relative: String,
    pub content: Vec<u8>,
    pub previous: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallReport {
    /// Paths that would change (dry-run / check) or did change (apply).
    pub differing: Vec<PathBuf>,
    /// Paths written on a successful apply.
    pub written: Vec<PathBuf>,
    pub receipt: PathBuf,
    pub check_ok: bool,
}

/// Install the five embedded DevMap skills under each destination.
pub fn install_devmap_skills(
    project_root: &Path,
    destinations: &[&str],
    dry_run: bool,
    check: bool,
) -> anyhow::Result<SkillInstallReport> {
    let root = project_root
        .canonicalize()
        .with_context(|| format!("skill destination root {}", project_root.display()))?;
    if !root.is_dir() {
        bail!(
            "{}: skill destination root is not a directory",
            root.display()
        );
    }
    if destinations.is_empty() || destinations.len() > MAX_DESTINATIONS {
        bail!("Skill install limit: 1–{MAX_DESTINATIONS} destinations");
    }
    if PLUGIN_SKILLS.len() > 256 {
        bail!("Skill install limit: at most 256 skills");
    }

    let mut rendered: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    for rel in destinations {
        validate_relative(&root, rel)?;
    }
    for (name, body) in PLUGIN_SKILLS {
        validate_skill_name(name)?;
        let mut text = (*body).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let content = text.into_bytes();
        if content.len() > SKILL_FILE_LIMIT {
            bail!("{name}: skill exceeds byte limit");
        }
        for rel in destinations {
            let relative = format!("{rel}/{name}/SKILL.md");
            let target = validate_relative(&root, &relative)?;
            rendered.insert(target, content.clone());
        }
    }
    let batch: usize = rendered.values().map(Vec::len).sum();
    if batch > MAX_BATCH_BYTES {
        bail!("Skill install exceeds 8 MiB batch limit");
    }

    let receipt = validate_relative(&root, RECEIPT_REL)?;
    let (plan, _hashes) = scaffold_plan(&root, &rendered, &receipt, dry_run || check)?;
    let differing: Vec<PathBuf> = plan.iter().map(|p| p.target.clone()).collect();

    if dry_run || check || rendered.is_empty() {
        return Ok(SkillInstallReport {
            check_ok: differing.is_empty(),
            differing,
            written: Vec::new(),
            receipt,
        });
    }

    let lock = root.join(LOCK_REL);
    acquire_lock(&lock)?;
    let result = (|| -> anyhow::Result<SkillInstallReport> {
        let (plan, hashes) = scaffold_plan(&root, &rendered, &receipt, false)?;
        let mut written = Vec::with_capacity(plan.len());
        for step in &plan {
            validate_relative(&root, &step.relative)?;
            if read_bounded(&step.target, SKILL_FILE_LIMIT)? != step.previous {
                bail!(
                    "{}: modified while installing skills",
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
        let prior = read_bounded(&receipt, SKILL_RECEIPT_LIMIT)?;
        if prior.as_deref() != Some(encoded.as_slice()) {
            if let Some(parent) = receipt.parent() {
                fs::create_dir_all(parent)?;
            }
            write_atomic_bytes(&receipt, &encoded)?;
        }
        Ok(SkillInstallReport {
            check_ok: true,
            differing: written.clone(),
            written,
            receipt,
        })
    })();
    let _ = fs::remove_dir(&lock);
    result
}

fn validate_skill_name(name: &str) -> anyhow::Result<()> {
    let ok = name.len() <= 64
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
        bail!("Invalid skill name: {name:?}");
    }
    Ok(())
}

fn validate_relative(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains(':')
        || relative.starts_with('/')
    {
        bail!("Invalid skill destination: {relative:?}");
    }
    let parts: Vec<&str> = relative.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        bail!("Invalid skill destination: {relative:?}");
    }
    let mut path = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        match path.symlink_metadata() {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    bail!("{}: refusing symlink in skill destination", path.display());
                }
                if index + 1 < parts.len() && !meta.is_dir() {
                    bail!("{}: skill parent is not a directory", path.display());
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(path)
}

fn scaffold_plan(
    root: &Path,
    rendered: &BTreeMap<PathBuf, Vec<u8>>,
    receipt: &Path,
    check_only: bool,
) -> anyhow::Result<(Vec<SkillInstallPlan>, BTreeMap<String, String>)> {
    let _ = validate_relative(root, RECEIPT_REL)?;
    let saved = match read_bounded(receipt, SKILL_RECEIPT_LIMIT)? {
        None => json!({"schema": 1, "files": {}}),
        Some(raw) => serde_json::from_slice(&raw)
            .map_err(|_| anyhow!("{}: invalid skill installation receipt", receipt.display()))?,
    };
    let schema = saved
        .get("schema")
        .and_then(Value::as_u64)
        .filter(|n| *n == 1);
    let files = saved.get("files").and_then(Value::as_object);
    if schema.is_none() || files.is_none() {
        // Reject JSON `true` masquerading as schema 1 via loose equality.
        bail!("{}: invalid skill installation receipt", receipt.display());
    }
    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in files.unwrap() {
        let digest = value
            .as_str()
            .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| {
                anyhow!(
                    "{}: invalid or oversized skill installation receipt",
                    receipt.display()
                )
            })?;
        hashes.insert(key.clone(), digest.to_ascii_lowercase());
    }
    if hashes.len() > MAX_RECEIPT_ENTRIES {
        bail!(
            "{}: skill installation receipt exceeds limit",
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
        let current = read_bounded(target, SKILL_FILE_LIMIT)?;
        if current.as_deref() != Some(content.as_slice()) {
            if !check_only {
                if let Some(existing) = &current {
                    let prior = hashes.get(&key).map(String::as_str);
                    let actual = sha256_hex(existing);
                    if prior != Some(actual.as_str()) {
                        bail!(
                            "{}: unmanaged or locally modified skill; preserve or move it before installing",
                            target.display()
                        );
                    }
                }
            }
            plan.push(SkillInstallPlan {
                target: target.clone(),
                relative: key.clone(),
                content: content.clone(),
                previous: current,
            });
        }
        hashes.insert(key, sha256_hex(content));
    }
    let encoded = serde_json::to_vec(&json!({"schema": 1, "files": hashes}))?;
    if hashes.len() > MAX_RECEIPT_ENTRIES || encoded.len() > SKILL_RECEIPT_LIMIT {
        bail!(
            "{}: skill installation receipt exceeds limit",
            receipt.display()
        );
    }
    Ok((plan, hashes))
}

fn acquire_lock(lock: &Path) -> anyhow::Result<()> {
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match fs::create_dir(lock) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if Instant::now() >= deadline {
                    bail!("Skill installer is busy: {}", lock.display());
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
        path.file_name().and_then(|n| n.to_str()).unwrap_or("skill"),
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

/// SHA-256 hex digest for the receipt format shared with the Python installer.
///
/// Delegates to [`crate::sha256`]. This was a second hand-rolled FIPS 180-4
/// implementation, kept "to avoid a new crate dependency" — a reason that had
/// already lapsed, since `sha2` was in the workspace for `dc-evidence`. Two
/// copies of one hash cannot be checked against each other by anything, and a
/// receipt is exactly where a disagreement would go unnoticed.
pub fn sha256_hex(data: &[u8]) -> String {
    crate::sha256::sha256_hex(data)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devmap-skills-{label}-{}-{:?}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The corpus contract, ported from the retired
    /// `tests/unit/test_devmap_skill_delivery.py`.
    ///
    /// That test compared two distributions of these five skills — the DevMap
    /// crate's own copy and DevCouncil's Python skill library — and asserted
    /// they were byte-identical. The Python library was deleted in 3286db5 and
    /// its Go replacement carries the 17 domain skills, not these, so there is
    /// now one distribution and the parity half is retired. What survives is
    /// what the parity check was protecting: that each shipped skill is
    /// addressable under the name a host is told to look for, and that its body
    /// still points at the command an agent runs first.
    #[test]
    fn every_shipped_skill_is_named_and_points_at_a_real_command() {
        let names: Vec<&str> = PLUGIN_SKILLS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                "devmap",
                "devmap-exploring",
                "devmap-debugging",
                "devmap-impact",
                "devmap-refactoring"
            ],
            "the shipped skill set changed; hosts are configured for these names"
        );

        for (name, body) in PLUGIN_SKILLS {
            validate_skill_name(name).unwrap_or_else(|err| panic!("{name}: {err}"));

            let front = body
                .strip_prefix("---\n")
                .and_then(|rest| rest.split_once("\n---\n"))
                .unwrap_or_else(|| panic!("{name}: SKILL.md has no terminated frontmatter"))
                .0;
            let declared = front
                .lines()
                .find_map(|line| line.strip_prefix("name:"))
                .map(str::trim)
                .unwrap_or_else(|| panic!("{name}: frontmatter declares no name"));
            assert_eq!(
                declared, *name,
                "a skill installed as {name}/SKILL.md that calls itself {declared} is \
                 invisible to a host asking for either"
            );
            let description = front
                .lines()
                .find_map(|line| line.strip_prefix("description:"))
                .map(str::trim)
                .unwrap_or_else(|| panic!("{name}: frontmatter declares no description"));
            assert!(
                !description.is_empty(),
                "{name}: the description is what a host matches on"
            );

            // `devmap paths --json` is the first command generated guidance
            // tells an agent to run; a skill that omits it sends the agent
            // straight to the fallback it exists to avoid.
            assert!(
                body.contains("devmap paths --json"),
                "{name}: no `devmap paths --json`"
            );
            // Two phrasings retired on purpose: guidance that forbids another
            // tool outright, and guidance that predicts breakage it cannot
            // observe. Both were removed from the corpus; neither may return.
            for banned in ["Do not use GitNexus", "will break"] {
                assert!(
                    !body.contains(banned),
                    "{name}: retired phrasing {banned:?} is back in the shipped skill"
                );
            }
        }
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn install_writes_five_skills_and_is_idempotent() {
        let dir = scratch("install");
        let report = install_devmap_skills(&dir, &[".agents/skills"], false, false).unwrap();
        assert_eq!(report.written.len(), 5);
        assert!((dir.join(".agents/skills/devmap/SKILL.md")).is_file());
        assert!((dir.join(RECEIPT_REL)).is_file());

        let again = install_devmap_skills(&dir, &[".agents/skills"], false, false).unwrap();
        assert!(again.written.is_empty());

        let check = install_devmap_skills(&dir, &[".agents/skills"], false, true).unwrap();
        assert!(check.check_ok);
        assert!(check.differing.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_unowned_local_edit() {
        let dir = scratch("unowned");
        install_devmap_skills(&dir, &[".agents/skills"], false, false).unwrap();
        let path = dir.join(".agents/skills/devmap/SKILL.md");
        fs::write(&path, "locally edited\n").unwrap();
        let err = install_devmap_skills(&dir, &[".agents/skills"], false, false).unwrap_err();
        assert!(
            err.to_string().contains("unmanaged or locally modified"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_timeout_when_busy() {
        let dir = scratch("lock");
        fs::create_dir(dir.join(LOCK_REL)).unwrap();
        let err = install_devmap_skills(&dir, &[".agents/skills"], false, false).unwrap_err();
        assert!(err.to_string().contains("busy"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_json_true_as_schema() {
        let dir = scratch("schema-true");
        fs::write(dir.join(RECEIPT_REL), r#"{"schema":true,"files":{}}"#).unwrap();
        let err = install_devmap_skills(&dir, &[".agents/skills"], false, false).unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid skill installation receipt"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
