//! The model's vocabulary and query-weight table, from the local Hugging Face
//! cache. Nothing here downloads. A fixture recorded from whatever a network
//! happened to serve is not a fixture, and an encoder that fetches weights on
//! a build machine is a different program from the one the operator ran.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

/// The model the script encoded with when `--model` was left alone.
pub const DEFAULT_MODEL: &str = "opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini";

/// The parity texts the script put in every header. `dcgrep` replays them
/// through its own WordPiece and refuses the build on one disagreement, so
/// this list is part of the file format, not a sample we are free to shrink.
pub const PARITY_TEXTS: &[&str] = &[
    "parse",
    "json",
    "parseJson",
    "parseJSONResponse",
    "HTTPServer",
    "http_server",
    "server2",
    "v2",
    "read_file",
    "readFile",
    "FILE",
    "unwrap_or_default",
    "tokenize",
    "zzqqxx",
    "sha256sum",
    "utf8",
    "supercalifragilisticexpialidocious",
    "ß",
    "ẞ",
    "ø",
    "Ø",
    "æ",
    "Æ",
    "ð",
    "þ",
    "đ",
    "ı",
    "ł",
    "Łódź",
    "œ",
    "Böse",
    "naïve",
    "café",
    "École",
    "Ångström",
    "žluťoučký",
    "Việt",
    "Tiếng",
    "ﬁle",
    "e\u{0301}cole",
    "n\u{0303}ino",
    "İstanbul",
    "日本語",
    "中文字符",
    "한국어",
    "снег",
    "Ελληνικά",
    "עברית",
    "العربية",
    "ไทย",
    "हिन्दी",
    "a\u{200b}b",
    "a\u{feff}b",
    "a\u{00a0}b",
    "a\u{0000}b",
    "a\u{000b}b",
    "🎉",
    "a🎉b",
    "→",
    "±",
    "§",
    "§8",
    "audit §e",
    "€",
    "℃",
    "a.b.c",
    "foo::bar",
    "x-=+",
    "don't",
    "e.g.",
    "U.S.A.",
    "path/to/file.rs",
    "https://example.com/a?b=c",
    "user@example.com",
    "#[derive(Debug)]",
    "{\"k\":[1,2]}",
    "// line",
    "全角。句点",
    "",
    " ",
    "   ",
    "\t\n",
    "  spaced  out  ",
];

/// The script's parity list. The last three entries are `a` repeated 99, 100
/// and 101 times — the per-word ceiling, on both sides of it — built rather
/// than pasted so a miscount cannot ship.
pub fn parity_texts() -> Vec<String> {
    let mut texts: Vec<String> = PARITY_TEXTS.iter().copied().map(str::to_string).collect();
    texts.push("a".repeat(99));
    texts.push("a".repeat(100));
    texts.push("a".repeat(101));
    texts
}

/// Where the hub cache lives. `HF_HUB_CACHE` is the hub directory itself;
/// `HF_HOME` is its parent.
pub fn hub_dir() -> PathBuf {
    if let Ok(cache) = std::env::var("HF_HUB_CACHE") {
        return PathBuf::from(cache);
    }
    if let Ok(cache) = std::env::var("HUGGINGFACE_HUB_CACHE") {
        return PathBuf::from(cache);
    }
    if let Ok(home) = std::env::var("HF_HOME") {
        return PathBuf::from(home).join("hub");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache/huggingface/hub")
}

/// The snapshot directory for `model_id`, via `refs/main`. Missing is an
/// error that names the cache path. This function never downloads.
pub fn snapshot(model_id: &str) -> Result<PathBuf, String> {
    let slug = format!("models--{}", model_id.replace('/', "--"));
    let repo = hub_dir().join(&slug);
    let pointer = repo.join("refs/main");
    let hash = std::fs::read_to_string(&pointer).map_err(|err| {
        format!(
            "no local snapshot for {model_id} ({}: {err}). \
             This encoder reads the Hugging Face cache and does not download; \
             the model has to be there already",
            pointer.display()
        )
    })?;
    let hash = hash.trim();
    if hash.is_empty() || hash.contains('/') || hash.contains("..") {
        return Err(format!("refs/main for {model_id} is not a snapshot id ({hash:?})"));
    }
    let snap = repo.join("snapshots").join(hash);
    if !snap.join("config.json").is_file() {
        return Err(format!(
            "snapshot {} for {model_id} has no config.json",
            snap.display()
        ));
    }
    Ok(snap)
}

/// `vocab.txt` in id order. A hole is not possible in that file — one line
/// is one id — and a duplicate would make the parity map refuse the build,
/// so it is refused here with the line named.
pub fn read_vocab(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("vocabulary {}: {err}", path.display()))?;
    let tokens: Vec<String> = text.lines().map(str::to_string).collect();
    if tokens.is_empty() {
        return Err(format!("vocabulary {} is empty", path.display()));
    }
    let mut seen = HashMap::<&str, usize>::new();
    for (i, token) in tokens.iter().enumerate() {
        if token.is_empty() {
            return Err(format!("vocabulary {} has an empty token at id {i}", path.display()));
        }
        if let Some(first) = seen.insert(token.as_str(), i) {
            return Err(format!(
                "vocabulary {} repeats {token:?} at ids {first} and {i}",
                path.display()
            ));
        }
    }
    Ok(tokens)
}

pub fn lookup(tokens: &[String]) -> HashMap<String, u32> {
    tokens
        .iter()
        .enumerate()
        .map(|(id, token)| (token.clone(), id as u32))
        .collect()
}

/// `idf.json` aligned to `tokens`. A missing file is the script's flat
/// fallback: every in-vocabulary token weighs 1.0, and the caller is told,
/// because a flat query side is a weaker ranking and must not be silent.
pub fn query_weights(idf: Option<&Path>, tokens: &[String]) -> Result<Vec<f64>, String> {
    let Some(path) = idf else {
        eprintln!(
            "note: no idf.json for this model; queries will weigh every known token equally."
        );
        return Ok(vec![1.0; tokens.len()]);
    };
    if !path.is_file() {
        eprintln!(
            "note: no idf.json for this model ({}); queries will weigh every known token equally.",
            path.display()
        );
        return Ok(vec![1.0; tokens.len()]);
    }
    let file = File::open(path).map_err(|err| format!("idf {}: {err}", path.display()))?;
    let table: HashMap<String, f64> = serde_json::from_reader(BufReader::new(file))
        .map_err(|err| format!("idf {} is not a token-to-weight object: {err}", path.display()))?;
    let index = lookup(tokens);
    let mut weights = vec![0.0; tokens.len()];
    for (token, weight) in table {
        if !weight.is_finite() {
            return Err(format!("idf weight for {token:?} is not finite ({weight})"));
        }
        if let Some(&id) = index.get(&token) {
            weights[id as usize] = weight;
        }
    }
    Ok(weights)
}

/// Rebuild a vocabulary in id order from an unordered map, filling a hole
/// with a name that cannot collide with a real token. This is the function
/// the script's self-test exists to pin: a vocabulary rebuilt in dict order
/// assigns every weight to the wrong token, and every score still looks
/// plausible.
pub fn densify(vocab: &HashMap<String, u32>) -> Result<Vec<String>, String> {
    if vocab.is_empty() {
        return Err("vocabulary is empty".into());
    }
    let size = vocab
        .values()
        .copied()
        .max()
        .ok_or("vocabulary is empty")? as usize
        + 1;
    let mut tokens: Vec<Option<String>> = vec![None; size];
    for (token, id) in vocab {
        let slot = tokens
            .get_mut(*id as usize)
            .ok_or_else(|| format!("token id {id} is past the vocabulary"))?;
        if slot.is_some() {
            return Err(format!("token id {id} is used twice"));
        }
        *slot = Some(token.clone());
    }
    Ok(tokens
        .into_iter()
        .enumerate()
        .map(|(i, token)| token.unwrap_or_else(|| format!("[unused_hole_{i}]")))
        .collect())
}
