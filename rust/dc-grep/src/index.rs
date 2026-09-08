//! Optional, bounded tgrep snapshots. The live walker still owns membership,
//! and a file is excluded only when its opened descriptor matches the version
//! read into the index. Missing evidence always means running the matcher.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tgrep_core::PostingEntry;
use tgrep_core::meta::{FileEvidence, file_version};
use tgrep_core::query::{build_query_plan, execute_plan};
use tgrep_core::reader::IndexReader;

use crate::{DEFAULT_MAX_FILE_BYTES, build_walker, open_for_search, resolve_roots, slashed};

const CACHE_SCHEMA: u32 = 1;
const MAX_INDEX_FILES: usize = 50_000;
const MAX_INDEX_POSTINGS: usize = 2_000_000;
const MAX_INDEX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_INDEX_DURATION: Duration = Duration::from_secs(30);
const MAX_CACHE_FILE_BYTES: u64 = 64 * 1024 * 1024;
const INDEX_FILES: &[&str] = &[
    "index.bin",
    "lookup.bin",
    "files.bin",
    "meta.json",
    "filestamps.json",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexRequest {
    pub root: PathBuf,
    /// Optional smaller file budget. Zero uses the built-in ceiling.
    #[serde(default)]
    pub max_files: usize,
}

#[derive(Debug, Serialize)]
pub struct IndexResponse {
    pub ok: bool,
    pub engine: &'static str,
    pub files_seen: usize,
    pub files_indexed: usize,
    pub files_unindexed: usize,
    pub walk_errors: usize,
    /// The walk finished; this does not assert every file was indexable.
    pub traversal_complete: bool,
    pub limit_reason: Option<&'static str>,
    pub input_bytes: u64,
    pub postings: usize,
    pub cache_warning: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchIndex {
    pub status: &'static str,
    pub reason: Option<String>,
    pub files_indexed: usize,
    pub files_filtered: u64,
    pub files_stale: u64,
}

impl SearchIndex {
    fn scan(reason: impl Into<String>) -> Self {
        Self {
            status: "scan",
            reason: Some(reason.into()),
            files_indexed: 0,
            files_filtered: 0,
            files_stale: 0,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Current {
    schema: u32,
    root: PathBuf,
    slot: String,
    files_indexed: usize,
}

/// The reader lock keeps an inactive slot from being recycled while a search
/// uses it. Locks are nonblocking: contention costs acceleration, never search.
pub(crate) struct Candidates {
    _lock: File,
    evidence: FileEvidence,
    absent: HashSet<String>,
}

impl Candidates {
    pub(crate) fn load(
        root: &Path,
        pattern: &str,
        case_insensitive: bool,
    ) -> (Option<Self>, SearchIndex) {
        // Unix ctime + inode catches rewrites with restored mtime. Other
        // platforms retain live search until equivalent evidence is available.
        if !cfg!(unix) {
            return (None, SearchIndex::scan("precise_file_versions_unavailable"));
        }
        // tgrep's HIR adapter and ripgrep's byte matcher differ for byte-mode
        // flags and Unicode case folding. Conservatively scan those patterns;
        // ordinary case-sensitive regexes still get trigram planning.
        if case_insensitive || pattern.contains("(?") {
            return (None, SearchIndex::scan("pattern_requires_live_scan"));
        }
        let plan = match build_query_plan(pattern, false) {
            Ok(plan) if !plan.is_match_all() => plan,
            _ => return (None, SearchIndex::scan("pattern_has_no_safe_trigrams")),
        };
        match Self::open(root, &plan) {
            Ok((candidates, count)) => (
                Some(candidates),
                SearchIndex {
                    status: "used",
                    reason: None,
                    files_indexed: count,
                    files_filtered: 0,
                    files_stale: 0,
                },
            ),
            Err(err) => (None, SearchIndex::scan(format!("index unavailable: {err}"))),
        }
    }

    fn open(root: &Path, plan: &tgrep_core::query::QueryPlan) -> Result<(Self, usize), String> {
        let cache = cache_directory(root, false)?;
        let lock = open_for_search(&cache.join("cache.lock")).map_err(error)?;
        lock.try_lock_shared().map_err(error)?;
        let current: Current = read_json(&cache.join("current.json"), 4096)?;
        if current.schema != CACHE_SCHEMA
            || current.root != root
            || !matches!(current.slot.as_str(), "slot-a" | "slot-b")
        {
            return Err("cache identity or schema differs; rebuild with dcgrep index".into());
        }
        let slot = cache.join(&current.slot);
        require_directory(&slot)?;
        require_directory(&slot.join("integrity"))?;
        bounded_regular_file(&slot.join("integrity/filestamps.json"), 16 * 1024)?;
        let integrity =
            tgrep_core::meta::read_file_evidence(&slot.join("integrity")).map_err(error)?;
        // Detect replaced, truncated, or edited artifacts before interpreting
        // their postings. The snapshots themselves are immutable while locked.
        for name in INDEX_FILES {
            let metadata = bounded_regular_file(&slot.join(name), MAX_CACHE_FILE_BYTES)?;
            if integrity.version(name) != Some(&file_version(&metadata)) {
                return Err(format!(
                    "{name} changed after publication; rebuild with dcgrep index"
                ));
            }
        }
        let reader = IndexReader::open(&slot).map_err(error)?;
        reader.validate_lookup()?;
        if reader.num_files() != current.files_indexed {
            return Err("index file count disagrees with its manifest".into());
        }
        let evidence = tgrep_core::meta::read_file_evidence(&slot).map_err(error)?;
        let matching: HashSet<u32> = execute_plan(plan, &|hash| reader.lookup_trigram(hash))
            .into_iter()
            .collect();
        let mut absent = HashSet::new();
        for (id, path) in reader.all_paths().iter().enumerate() {
            let id = u32::try_from(id).map_err(error)?;
            if !matching.contains(&id) && evidence.version(path).is_some() {
                absent.insert(path.clone());
            }
        }
        Ok((
            Self {
                _lock: lock,
                evidence,
                absent,
            },
            reader.num_files(),
        ))
    }

    pub(crate) fn excludes(
        &self,
        path: &str,
        metadata: &Metadata,
        stats: &mut SearchIndex,
    ) -> bool {
        let Some(version) = self.evidence.version(path) else {
            return false;
        };
        if *version != file_version(metadata) {
            stats.files_stale += 1;
            return false;
        }
        if self.absent.contains(path) {
            stats.files_filtered += 1;
            return true;
        }
        false
    }
}

/// Build a bounded snapshot with tgrep's extractor and on-disk writer. Using
/// the existing safe walker/open boundary avoids introducing a second file
/// admission policy or a library read that follows replacement symlinks.
pub fn build_index(request: &IndexRequest) -> Result<IndexResponse, String> {
    if request.max_files > MAX_INDEX_FILES {
        return Err(format!(
            "max_files exceeds the {MAX_INDEX_FILES}-file index ceiling"
        ));
    }
    let max_files = if request.max_files == 0 {
        MAX_INDEX_FILES
    } else {
        request.max_files
    };
    build_with_limits(
        request,
        max_files,
        MAX_INDEX_POSTINGS,
        MAX_INDEX_INPUT_BYTES,
        MAX_INDEX_DURATION,
    )
}

fn build_with_limits(
    request: &IndexRequest,
    max_files: usize,
    max_postings: usize,
    max_bytes: u64,
    max_duration: Duration,
) -> Result<IndexResponse, String> {
    let (root, _) = resolve_roots(&request.root, "")?;
    if !root.is_dir() {
        return Err("index root must be a directory".into());
    }
    let cache = cache_directory(&root, true)?;
    let lock = open_cache_writer(&cache.join("cache.lock"), true)?;
    lock.try_lock()
        .map_err(|err| format!("index busy or locking unavailable: {err}"))?;
    let pointer = cache.join("current.json");
    let (previous, cache_warning) = if pointer.try_exists().map_err(error)? {
        match read_json::<Current>(&pointer, 4096) {
            Ok(current) => (Some(current), None),
            Err(err) => (
                None,
                Some(format!("replaced unreadable cache pointer: {err}")),
            ),
        }
    } else {
        (None, None)
    };
    let slot_name = if previous.as_ref().is_some_and(|p| p.slot == "slot-a") {
        "slot-b"
    } else {
        "slot-a"
    };
    let slot = cache.join(slot_name);
    if slot.try_exists().map_err(error)? {
        require_directory(&slot)?;
        // Fixed private cache slots, never a caller-supplied deletion target.
        // The other slot remains published throughout this rebuild.
        fs::remove_dir_all(&slot).map_err(error)?;
    }
    fs::create_dir(&slot).map_err(error)?;

    let started = Instant::now();
    let mut paths = Vec::new();
    let mut inverted: HashMap<u32, Vec<PostingEntry>> = HashMap::new();
    let mut evidence = FileEvidence::default();
    let mut response = IndexResponse {
        ok: true,
        engine: "tgrep-core",
        files_seen: 0,
        files_indexed: 0,
        files_unindexed: 0,
        walk_errors: 0,
        traversal_complete: true,
        limit_reason: None,
        input_bytes: 0,
        postings: 0,
        cache_warning,
    };
    for entry in build_walker(&root, true)?.build() {
        if started.elapsed() >= max_duration {
            response.limit_reason = Some("duration");
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                response.walk_errors += 1;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        response.files_seen += 1;
        if paths.len() >= max_files {
            response.limit_reason = Some("files");
            break;
        }
        let Some(path) = entry.path().strip_prefix(&root).ok().and_then(slashed) else {
            continue;
        };
        let file = match open_for_search(entry.path()) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let metadata = match file.metadata() {
            Ok(metadata) if metadata.is_file() && metadata.len() < DEFAULT_MAX_FILE_BYTES => {
                metadata
            }
            _ => continue,
        };
        if response.input_bytes.saturating_add(metadata.len()) > max_bytes {
            response.limit_reason = Some("input_bytes");
            break;
        }
        let before = file_version(&metadata);
        if !before.is_trusted() {
            continue;
        }
        let mut bytes = Vec::new();
        let read = (&file).take(DEFAULT_MAX_FILE_BYTES).read_to_end(&mut bytes);
        response.input_bytes += bytes.len() as u64;
        if bytes.len() as u64 >= DEFAULT_MAX_FILE_BYTES || response.input_bytes > max_bytes {
            response.limit_reason = Some("input_bytes");
            break;
        }
        if read.is_err() {
            continue;
        }
        // Only unchanged plain UTF-8 is eligible for negative evidence. Other
        // encodings, NULs, and BOMs remain the live searcher's responsibility.
        if bytes.contains(&0)
            || bytes.starts_with(&[0xef, 0xbb, 0xbf])
            || std::str::from_utf8(&bytes).is_err()
        {
            continue;
        }
        let trigrams = tgrep_core::trigram::extract_merged_masks(&bytes);
        let after = match file.metadata() {
            Ok(meta) => file_version(&meta),
            Err(_) => continue,
        };
        if before != after {
            continue;
        }
        if response.postings.saturating_add(trigrams.len()) > max_postings {
            response.limit_reason = Some("postings");
            break;
        }
        let file_id = u32::try_from(paths.len()).map_err(error)?;
        response.postings += trigrams.len();
        for (hash, masks) in trigrams {
            inverted.entry(hash).or_default().push(PostingEntry {
                file_id,
                loc_mask: masks.loc_mask,
                next_mask: masks.next_mask,
            });
        }
        evidence.insert_verified(path.clone(), before.stamp().clone(), None, Some(before));
        paths.push(path);
    }
    response.files_indexed = paths.len();
    response.files_unindexed = response.files_seen - paths.len();
    response.traversal_complete = response.limit_reason.is_none();
    tgrep_core::builder::write_index_from_snapshot(
        &root,
        &slot,
        &paths,
        &inverted,
        response.traversal_complete && response.files_unindexed == 0 && response.walk_errors == 0,
    )
    .map_err(error)?;
    tgrep_core::meta::write_file_evidence(&evidence, &slot).map_err(error)?;
    drop(inverted);
    let check = IndexReader::open(&slot).map_err(error)?;
    check.validate_lookup()?;
    if check.num_files() != paths.len() {
        return Err("written index lost file entries".into());
    }
    drop(check);
    let mut integrity = FileEvidence::default();
    for name in INDEX_FILES {
        let file = open_cache_writer(&slot.join(name), false)?;
        file.sync_all().map_err(error)?;
        let version = file_version(&bounded_regular_file(
            &slot.join(name),
            MAX_CACHE_FILE_BYTES,
        )?);
        integrity.insert_verified((*name).into(), version.stamp().clone(), None, Some(version));
    }
    fs::create_dir(slot.join("integrity")).map_err(error)?;
    tgrep_core::meta::write_file_evidence(&integrity, &slot.join("integrity")).map_err(error)?;
    open_cache_writer(&slot.join("integrity/filestamps.json"), false)?
        .sync_all()
        .map_err(error)?;
    let pending = cache.join("current.tmp");
    match fs::remove_file(&pending) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(error(err)),
    }
    let mut publish = File::create_new(&pending).map_err(error)?;
    let current = Current {
        schema: CACHE_SCHEMA,
        root,
        slot: slot_name.into(),
        files_indexed: paths.len(),
    };
    publish
        .write_all(&serde_json::to_vec(&current).map_err(error)?)
        .map_err(error)?;
    publish.sync_all().map_err(error)?;
    drop(publish);
    fs::rename(pending, pointer).map_err(error)?;
    Ok(response)
}

fn error(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn require_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "cache path must be a real directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn cache_directory(root: &Path, create: bool) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for part in [".devcouncil", "dcgrep"] {
        path.push(part);
        if create {
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(error(err)),
            }
        }
        require_directory(&path)?;
    }
    Ok(path)
}

fn open_cache_writer(path: &Path, create: bool) -> Result<File, String> {
    let mut options = File::options();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(error)?;
    if !file.metadata().map_err(error)?.is_file() {
        return Err("cache output is not a regular file".into());
    }
    Ok(file)
}

fn bounded_regular_file(path: &Path, limit: u64) -> Result<Metadata, String> {
    let metadata = open_for_search(path)
        .map_err(error)?
        .metadata()
        .map_err(error)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "cache file is not regular or exceeds {limit} bytes: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<T, String> {
    bounded_regular_file(path, limit)?;
    let mut bytes = Vec::new();
    open_for_search(path)
        .map_err(error)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err("cache JSON exceeded its read bound".into());
    }
    serde_json::from_slice(&bytes).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::{IndexRequest, MAX_INDEX_DURATION, build_with_limits};
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn resource_caps_publish_partial_evidence_without_hiding_unindexed_matches() {
        let root = std::env::temp_dir().join(format!(
            "dcgrep-budget-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("hit.rs"), "needle\n").unwrap();
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
        };
        for (postings, bytes, duration, reason) in [
            (0, 100, MAX_INDEX_DURATION, "postings"),
            (100, 0, MAX_INDEX_DURATION, "input_bytes"),
            (100, 100, Duration::ZERO, "duration"),
        ] {
            let built = build_with_limits(&request, 100, postings, bytes, duration).unwrap();
            assert_eq!(built.files_indexed, 0);
            assert_eq!(built.limit_reason, Some(reason));
            assert!(!built.traversal_complete);
            let search = serde_json::from_value(serde_json::json!({
                "root": root, "pattern": "needle"
            }))
            .unwrap();
            assert_eq!(crate::search(&search).unwrap().count, 1, "{reason}");
        }
        fs::remove_dir_all(root).unwrap();
    }
}
