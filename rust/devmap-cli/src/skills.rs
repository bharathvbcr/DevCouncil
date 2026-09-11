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

/// SHA-256 hex digest. Hand-rolled to avoid a new crate dependency for the
/// receipt format shared with the Python installer.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn sha256(mut data: &[u8]) -> [u8; 32] {
    // FIPS 180-4 SHA-256. Compact implementation for receipt hashing only.
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_len = (data.len() as u64).saturating_mul(8);
    while data.len() >= 64 {
        process_block(&mut h, &K, data);
        data = &data[64..];
    }
    let mut rem = data.to_vec();
    rem.push(0x80);
    while rem.len() % 64 != 56 {
        rem.push(0);
    }
    rem.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in rem.as_chunks::<64>().0 {
        process_block(&mut h, &K, chunk);
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn process_block(h: &mut [u32; 8], k: &[u32; 64], block: &[u8]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
        (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(k[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
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
