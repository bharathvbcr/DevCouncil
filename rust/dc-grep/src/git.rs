//! Read-only Git repository metadata helpers for `dc-grep`.
//!
//! Reads `.git/config` (`core.ignorecase` and layout gates) and `.git/index`
//! (tracked regular-file paths when `include_git_tracked` is requested) through
//! `O_NOFOLLOW | O_NONBLOCK` descriptors verified to be regular files.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::{contained_relative, open_for_search};

const MAX_POINTER_BYTES: u64 = 4096;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_INDEX_ENTRIES: u32 = 500_000;
const INDEX_HEADER_LEN: usize = 12;
const MIN_INDEX_ENTRY_BYTES: u64 = 44;
const MAX_EXTENSIONS: usize = 64;

const GITLINK_MODE_MASK: u32 = 0o170000;
const REGULAR_FILE_MODE: u32 = 0o100000;

/// Opens `path` without following a trailing symlink or blocking on a FIFO,
/// then confirms via the opened descriptor's metadata that it is a regular file.
fn open_safe_regular(path: &Path) -> io::Result<File> {
    let file = open_for_search(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    Ok(file)
}

fn read_small_regular(path: &Path) -> Result<String, String> {
    let file = open_safe_regular(path).map_err(|err| format!("unreadable {path:?}: {err}"))?;
    let meta = file
        .metadata()
        .map_err(|err| format!("unreadable {path:?}: {err}"))?;
    if meta.len() > MAX_POINTER_BYTES {
        return Err(format!("{path:?} exceeds {MAX_POINTER_BYTES}-byte limit"));
    }
    let mut raw = Vec::new();
    file.take(MAX_POINTER_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|err| format!("unreadable {path:?}: {err}"))?;
    if raw.len() as u64 > MAX_POINTER_BYTES || raw.contains(&0) {
        return Err(format!("{path:?} is invalid or oversized"));
    }
    String::from_utf8(raw).map_err(|_| format!("{path:?} is not valid UTF-8"))
}

fn resolve_common_dir(git_dir: &Path) -> Result<PathBuf, String> {
    let commondir_path = git_dir.join("commondir");
    match fs::symlink_metadata(&commondir_path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(git_dir.to_path_buf()),
        Err(err) => Err(format!("could not inspect {commondir_path:?}: {err}")),
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("git commondir symlink is unsupported".to_string())
        }
        Ok(meta) if meta.is_file() => {
            let text = read_small_regular(&commondir_path)?;
            let rel = text.trim();
            if rel.is_empty() {
                return Err("git commondir file is empty".to_string());
            }
            let target = Path::new(rel);
            let joined = if target.is_absolute() {
                target.to_path_buf()
            } else {
                git_dir.join(target)
            };
            let canonical = joined
                .canonicalize()
                .map_err(|err| format!("git commondir is unreadable: {err}"))?;
            if !canonical.is_dir() {
                return Err("git commondir is not a directory".to_string());
            }
            Ok(canonical)
        }
        Ok(_) => Err("git commondir has unsupported file type".to_string()),
    }
}

/// Resolves `(git_dir, common_dir)` for `root`, or `None` when `root/.git`
/// does not exist.
pub(crate) fn resolve_git_dirs(root: &Path) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let dot_git = root.join(".git");
    let meta = match fs::symlink_metadata(&dot_git) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("could not inspect .git: {err}")),
    };
    if meta.file_type().is_symlink() {
        return Err(".git symlink is unsupported".to_string());
    }
    let git_dir = if meta.is_dir() {
        dot_git
            .canonicalize()
            .map_err(|err| format!(".git is unreadable: {err}"))?
    } else if meta.is_file() {
        let text = read_small_regular(&dot_git)?;
        let line = text.trim_end_matches(['\r', '\n']);
        let Some(raw_target) = line.strip_prefix("gitdir:") else {
            return Err("unrecognized .git file format".to_string());
        };
        let trimmed = raw_target.trim();
        if trimmed.is_empty() {
            return Err("empty gitdir pointer".to_string());
        }
        let target = Path::new(trimmed);
        let joined = if target.is_absolute() {
            target.to_path_buf()
        } else {
            root.join(target)
        };
        let canonical = joined
            .canonicalize()
            .map_err(|err| format!("linked gitdir is unreadable: {err}"))?;
        if !canonical.is_dir() {
            return Err("linked gitdir is not a directory".to_string());
        }
        canonical
    } else {
        return Err(".git has unsupported file type".to_string());
    };
    let common_dir = resolve_common_dir(&git_dir)?;
    Ok(Some((git_dir, common_dir)))
}

fn config_bool(raw: &str) -> Option<bool> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("yes")
        || trimmed.eq_ignore_ascii_case("on")
        || trimmed == "1"
    {
        Some(true)
    } else if trimmed.eq_ignore_ascii_case("false")
        || trimmed.eq_ignore_ascii_case("no")
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed == "0"
        || trimmed.is_empty()
    {
        Some(false)
    } else {
        None
    }
}

/// Strips unquoted `#` / `;` comments from one physical line before line
/// continuation is evaluated, tracking double-quote and escape state across
/// continued physical lines so `# comment \` can never continue into the next
/// line.
fn strip_physical_comment<'a>(line: &'a str, in_quotes: &mut bool) -> &'a str {
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if *in_quotes {
            match ch {
                '\\' => escaped = true,
                '"' => *in_quotes = false,
                _ => {}
            }
        } else {
            match ch {
                '"' => *in_quotes = true,
                '#' | ';' => return &line[..idx],
                _ => {}
            }
        }
    }
    line
}

fn ends_with_unescaped_backslash(s: &str) -> bool {
    let backslashes = s.bytes().rev().take_while(|&b| b == b'\\').count();
    backslashes % 2 == 1
}

fn find_section_close(after_open: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in after_open.char_indices() {
        if in_quotes {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_quotes = false;
            }
        } else if ch == '"' {
            in_quotes = true;
        } else if ch == ']' {
            return Some(idx);
        }
    }
    None
}

fn unquote_config_value(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(String::new());
    }
    let mut out = String::with_capacity(raw.len());
    let mut in_quotes = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if escaped {
            escaped = false;
            out.push(match ch {
                'n' => '\n',
                't' => '\t',
                'b' => '\x08',
                other => other,
            });
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => in_quotes = !in_quotes,
            _ => out.push(ch),
        }
    }
    if in_quotes || escaped {
        return None;
    }
    Some(out)
}

fn visit_git_config(
    path: &Path,
    mut visit: impl FnMut(&str, &str, Option<&str>) -> bool,
) -> Result<(), String> {
    let file = match open_safe_regular(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("git config {path:?} is unreadable: {err}")),
    };
    let meta = file
        .metadata()
        .map_err(|err| format!("git config {path:?} is unreadable: {err}"))?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return Err(format!("git config {path:?} is invalid or oversized"));
    }
    let mut raw = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|err| format!("git config {path:?} could not be read: {err}"))?;
    if raw.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!("git config {path:?} exceeds size limit"));
    }
    let text =
        String::from_utf8(raw).map_err(|_| format!("git config {path:?} is not valid UTF-8"))?;

    let mut section = String::new();
    let mut lines = text.lines().peekable();
    while let Some(first) = lines.next() {
        let mut in_quotes = false;
        let first_stripped = strip_physical_comment(first, &mut in_quotes);
        let mut combined = first_stripped.to_string();
        while ends_with_unescaped_backslash(&combined) {
            combined.pop();
            let Some(next) = lines.next() else {
                break;
            };
            let next_stripped = strip_physical_comment(next, &mut in_quotes);
            combined.push_str(next_stripped);
        }
        let mut rest = combined.trim();
        while !rest.is_empty() {
            if let Some(after_open) = rest.strip_prefix('[') {
                let Some(close_idx) = find_section_close(after_open) else {
                    return Err("unclosed git config section header".to_string());
                };
                let header = after_open[..close_idx].trim();
                let name_end = header
                    .find(|c: char| c.is_ascii_whitespace() || c == '"')
                    .unwrap_or(header.len());
                let raw_section = header[..name_end].to_ascii_lowercase();
                if raw_section.is_empty() {
                    return Err("empty git config section name".to_string());
                }
                let has_subsection = !header[name_end..].trim().is_empty();
                section =
                    if has_subsection && !matches!(raw_section.as_str(), "include" | "includeif") {
                        format!("{raw_section}\0sub")
                    } else {
                        raw_section
                    };
                if !visit(&section, "", None) {
                    return Ok(());
                }
                rest = after_open[close_idx + 1..].trim();
                continue;
            }
            let (key, value) = match rest.split_once('=') {
                Some((k, v)) => {
                    let parsed = unquote_config_value(v)
                        .ok_or_else(|| "malformed git config value".to_string())?;
                    (k.trim().to_ascii_lowercase(), Some(parsed))
                }
                None => (rest.trim().to_ascii_lowercase(), None),
            };
            if !key.is_empty() && !visit(&section, &key, value.as_deref()) {
                return Ok(());
            }
            break;
        }
    }
    Ok(())
}

/// Returns `true` when `core.ignorecase` resolves to true in `.git/config`
/// (or `config.worktree` if present). Defaults to `false` when unset or when
/// the repository metadata is absent or unreadable.
pub(crate) fn read_ignore_case(root: &Path) -> bool {
    let Ok(Some((git_dir, common_dir))) = resolve_git_dirs(root) else {
        return false;
    };
    let mut ignore_case = false;
    if visit_git_config(&common_dir.join("config"), |section, key, value| {
        if section == "core" && key == "ignorecase" {
            match value {
                None => ignore_case = true,
                Some(v) => {
                    if let Some(b) = config_bool(v) {
                        ignore_case = b;
                    }
                }
            }
        }
        true
    })
    .is_err()
    {
        return false;
    }
    let worktree_cfg = git_dir.join("config.worktree");
    if fs::symlink_metadata(&worktree_cfg).is_ok()
        && visit_git_config(&worktree_cfg, |section, key, value| {
            if section == "core" && key == "ignorecase" {
                match value {
                    None => ignore_case = true,
                    Some(v) => {
                        if let Some(b) = config_bool(v) {
                            ignore_case = b;
                        }
                    }
                }
            }
            true
        })
        .is_err()
    {
        return false;
    }
    ignore_case
}

struct Cursor {
    inner: BufReader<File>,
    pos: u64,
    end: u64,
}

impl Cursor {
    fn take(&mut self, buf: &mut [u8]) -> Result<(), String> {
        let want = buf.len() as u64;
        if self.pos + want > self.end {
            return Err("an index entry or extension runs past the end of the index".to_string());
        }
        self.inner
            .read_exact(buf)
            .map_err(|err| format!("the git index could not be read: {err}"))?;
        self.pos += want;
        Ok(())
    }

    fn byte(&mut self) -> Result<u8, String> {
        let mut one = [0u8; 1];
        self.take(&mut one)?;
        Ok(one[0])
    }

    fn append_until_nul(&mut self, out: &mut Vec<u8>) -> Result<usize, String> {
        let mut added = 0usize;
        loop {
            let b = self.byte()?;
            if b == 0 {
                return Ok(added);
            }
            if out.len() >= 16 * 1024 {
                return Err("git index entry path exceeds 16 KiB".to_string());
            }
            out.push(b);
            added += 1;
        }
    }

    fn skip(&mut self, mut bytes: u64) -> Result<(), String> {
        if self.pos + bytes > self.end {
            return Err("a git index extension runs past the end of the index".to_string());
        }
        let mut buf = [0u8; 4096];
        while bytes > 0 {
            let chunk = usize::try_from(bytes.min(buf.len() as u64)).unwrap_or(buf.len());
            self.inner
                .read_exact(&mut buf[..chunk])
                .map_err(|err| format!("the git index could not be read: {err}"))?;
            self.pos += chunk as u64;
            bytes -= chunk as u64;
        }
        Ok(())
    }
}

/// Reads tracked regular-file repository-relative paths from `.git/index`.
///
/// Fails closed on unsupported configurations (bare, split index, sparse
/// checkout, worktree-local config, `[include]`/`[includeIf]`, non-UTF-8 paths,
/// or corrupt headers) so callers never mistake partial index coverage for the
/// full repository.
pub(crate) fn read_tracked_regular_paths(root: &Path) -> Result<Vec<String>, String> {
    let Some((git_dir, common_dir)) = resolve_git_dirs(root)? else {
        return Ok(Vec::new());
    };
    if fs::symlink_metadata(git_dir.join("config.worktree")).is_ok()
        || fs::symlink_metadata(git_dir.join("info").join("sparse-checkout")).is_ok()
        || fs::symlink_metadata(common_dir.join("info").join("sparse-checkout")).is_ok()
    {
        return Err(
            "sparse checkout or worktree config is unsupported by direct index read".into(),
        );
    }

    let mut hash_len = 20usize;
    let mut unsupported = false;
    let mut bare = false;
    let mut split_index = false;
    let mut sparse_checkout = false;
    let mut index_sparse = false;
    visit_git_config(&common_dir.join("config"), |section, key, value| {
        if section == "include" || section == "includeif" {
            unsupported = true;
            return false;
        }
        if section == "core" {
            match key {
                "excludesfile" | "worktree" => {
                    unsupported = true;
                    return false;
                }
                "bare" => bare = value.is_none_or(|v| config_bool(v).unwrap_or(true)),
                "splitindex" => split_index = value.is_none_or(|v| config_bool(v).unwrap_or(true)),
                "sparsecheckout" => {
                    sparse_checkout = value.is_none_or(|v| config_bool(v).unwrap_or(true))
                }
                _ => {}
            }
        } else if section == "index" && key == "sparse" {
            index_sparse = value.is_none_or(|v| config_bool(v).unwrap_or(true));
        } else if section == "extensions" && key == "objectformat" {
            match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
                Some("sha256") => hash_len = 32,
                Some("sha1") => hash_len = 20,
                _ => {
                    unsupported = true;
                    return false;
                }
            }
        }
        true
    })?;
    if unsupported || bare || split_index || sparse_checkout || index_sparse {
        return Err("repository config requires git fallback".to_string());
    }

    let index_path = git_dir.join("index");
    let file = match open_safe_regular(&index_path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("git index could not be opened: {err}")),
    };
    let meta = file
        .metadata()
        .map_err(|err| format!("git index metadata could not be read: {err}"))?;
    if !meta.is_file() || meta.len() > MAX_INDEX_BYTES {
        return Err("git index is invalid or exceeds size limit".to_string());
    }
    let file_len = meta.len();
    let end = file_len
        .checked_sub(hash_len as u64)
        .ok_or_else(|| "git index is smaller than checksum trailer".to_string())?;
    let mut cur = Cursor {
        inner: BufReader::new(file),
        pos: 0,
        end,
    };

    let mut head = [0u8; INDEX_HEADER_LEN];
    cur.take(&mut head)?;
    if &head[..4] != b"DIRC" {
        return Err("git index header magic is invalid".to_string());
    }
    let version = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
    if !matches!(version, 2..=4) {
        return Err(format!("unsupported git index version {version}"));
    }
    let entries = u32::from_be_bytes([head[8], head[9], head[10], head[11]]);
    if entries > MAX_INDEX_ENTRIES {
        return Err("git index entry count exceeds ceiling".to_string());
    }
    if u64::from(entries).saturating_mul(MIN_INDEX_ENTRY_BYTES) > file_len {
        return Err("git index header entry count exceeds file size".to_string());
    }

    let fixed = 40 + hash_len + 2;
    let mut fields = vec![0u8; fixed];
    let mut path_buf: Vec<u8> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for _ in 0..entries {
        cur.take(&mut fields)?;
        let mode = u32::from_be_bytes([fields[24], fields[25], fields[26], fields[27]]);
        let flags = u16::from_be_bytes([fields[fixed - 2], fields[fixed - 1]]);
        let mut fixed_total = fixed as u64;
        if flags & 0x4000 != 0 {
            if version < 3 {
                return Err("extended flags in version 2 git index".to_string());
            }
            let mut extended = [0u8; 2];
            cur.take(&mut extended)?;
            let ext_flags = u16::from_be_bytes(extended);
            if ext_flags & 0x4000 != 0 {
                return Err("sparse checkout skip-worktree bit is unsupported".to_string());
            }
            fixed_total += 2;
        }
        let name_len = usize::from(flags & 0x0FFF);
        if version < 4 {
            let long = name_len >= 0x0FFF;
            path_buf.clear();
            let path_len = if !long {
                if name_len == 0 {
                    return Err("empty path in git index".to_string());
                }
                path_buf.resize(name_len, 0);
                cur.take(&mut path_buf)?;
                name_len
            } else {
                let n = cur.append_until_nul(&mut path_buf)?;
                if n == 0 {
                    return Err("empty path in git index".to_string());
                }
                n
            };
            let size = (fixed_total + path_len as u64 + 8) & !7;
            let already = fixed_total + path_len as u64 + u64::from(long);
            let pad = size
                .checked_sub(already)
                .ok_or_else(|| "mis-sized git index entry".to_string())?;
            let mut padding = vec![0u8; usize::try_from(pad).unwrap_or(8)];
            cur.take(&mut padding)?;
            if padding.iter().any(|b| *b != 0) {
                return Err("non-NUL padding in git index entry".to_string());
            }
        } else {
            let mut b = cur.byte()?;
            let mut strip = usize::from(b & 0x7F);
            let mut bytes = 1;
            while b & 0x80 != 0 {
                bytes += 1;
                if bytes > 5 {
                    return Err("invalid strip count in v4 git index".to_string());
                }
                b = cur.byte()?;
                strip = ((strip + 1) << 7) | usize::from(b & 0x7F);
            }
            if strip > path_buf.len() {
                return Err("v4 git index entry strips past previous path".to_string());
            }
            path_buf.truncate(path_buf.len() - strip);
            cur.append_until_nul(&mut path_buf)?;
            if path_buf.is_empty() {
                return Err("empty path in v4 git index".to_string());
            }
        }

        // Only regular files are searched (`0o100xxx`); symlinks (`0o120000`)
        // and gitlinks (`0o160000`) are skipped by working-tree grep.
        if (mode & GITLINK_MODE_MASK) != REGULAR_FILE_MODE {
            continue;
        }
        let path = std::str::from_utf8(&path_buf)
            .map_err(|_| "non-UTF-8 path in git index".to_string())?;
        if !contained_relative(path) {
            return Err("unsafe path in git index".to_string());
        }
        if !seen.contains(path) {
            seen.insert(path.to_string());
        }
    }

    let mut extensions = 0usize;
    while cur.pos < end {
        extensions += 1;
        if extensions > MAX_EXTENSIONS {
            return Err("too many extensions in git index".to_string());
        }
        let mut ext = [0u8; 8];
        cur.take(&mut ext)?;
        if !ext[0].is_ascii_alphabetic() {
            return Err("invalid extension signature in git index".to_string());
        }
        if matches!(&ext[..4], b"link" | b"sdir") {
            return Err("split or sparse git index requires git fallback".to_string());
        }
        let size = u64::from(u32::from_be_bytes([ext[4], ext[5], ext[6], ext[7]]));
        cur.skip(size)?;
    }
    if cur.pos != end {
        return Err("git index extensions end past checksum".to_string());
    }

    Ok(seen.into_iter().collect())
}
