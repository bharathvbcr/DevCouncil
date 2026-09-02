use serde::{Deserialize, Serialize};

pub struct Budget;

impl Budget {
    pub const SEARCH: u32 = 2000;
    pub const DEPS: u32 = 2000;
    pub const DEAD: u32 = 2000;
    pub const MANIFEST: u32 = 2000;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request<Q> {
    pub query: Q,
    pub token_budget: u32, // T1-T4: required token budget primitive
    pub min_confidence: f32,
    pub max_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionAvailability {
    Available,
    Unavailable { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response<T> {
    pub items: Vec<T>,
    pub shown: u32,
    pub hidden: u32,
    pub total: u32, // what a complete answer would have held
    pub truncated: bool,
    pub tokens_used: u32,
    pub resolution: ResolutionAvailability,
}

/// What a map query cost, against what answering it by reading files would have.
///
/// Every number here is a count of bytes divided by [`BYTES_PER_TOKEN`], not a
/// tokenizer's output. That is an estimate and is labelled one; a real count
/// depends on the model doing the reading, and quoting a precise-looking
/// figure derived from a divisor would be a fabricated precision.
///
/// The comparison is deliberately conservative. `files_tokens` is the cost of
/// reading only the files the map *already named* — it does not charge the
/// alternative for the work of finding them, which without an index means a
/// grep over the tree and reading candidates that turn out not to match. So the
/// reported saving is a floor, not a best case, and the field names say which
/// side is which rather than presenting a single triumphant ratio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavingsReport {
    /// How token counts here were derived. Always an estimate.
    pub basis: String,
    /// Files in the latest generation.
    pub indexed_files: usize,
    /// Bytes of indexed source that could be read from disk.
    pub corpus_bytes: u64,
    /// Indexed files that could not be read to size them — deleted, moved, or
    /// unreadable since the generation was built. Carried so `corpus_bytes` is
    /// never mistaken for a complete measurement of the tree.
    pub corpus_files_unreadable: usize,
    /// Size of the artifact agents are instructed to open, when it exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_map_bytes: Option<u64>,
    /// Present only when a query was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<QuerySavings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuerySavings {
    pub query: String,
    /// Symbols the query returned within its budget.
    pub hits: usize,
    /// What the map's answer cost, as the engine itself accounted for it.
    pub answer_tokens: u32,
    /// Distinct files the answer pointed into.
    pub files_named: usize,
    /// Bytes of those files. The alternative this is compared against is
    /// "read the files the map named" — nothing cheaper would answer the same
    /// question, and anything an unindexed reader did would cost more.
    pub files_bytes: u64,
    /// Files among those named that could not be read to size them.
    pub files_unreadable: usize,
}

/// How a symbol differs between the indexed file and a candidate buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewChange {
    /// Not in the indexed file. Nothing can be depending on it yet.
    Added,
    /// In the indexed file, absent from the buffer. Callers break.
    Removed,
    /// Declaration changed. Callers may break — this is the interesting case.
    SignatureChanged,
    /// Same declaration, different body. Callers still compile; behaviour moved.
    BodyChanged,
    /// Changed, and nothing could separate the declaration from the body —
    /// the grammar exposes no body field on this construct. Grouped with the
    /// caller-affecting changes, because reporting a break that did not happen
    /// costs a reader a look, and missing one costs them a build.
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewSymbol {
    pub symbol_name: String,
    pub qualified_name: String,
    pub kind: String,
    pub change: PreviewChange,
    /// The indexed declaration, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    /// The buffer's declaration, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewCaller {
    /// The symbol in the edited file that this call targets.
    pub target_symbol: String,
    pub caller_file: String,
    pub caller_symbol: String,
    pub confidence: f32,
}

/// What a candidate edit would do to the graph, computed without writing it.
///
/// The delta is withheld entirely when the buffer does not parse. A file that
/// fails to parse yields no symbols, so a naive diff reports every symbol in it
/// as removed and every caller as breaking — the most alarming possible output,
/// produced by a typo. `delta_available` is the gate, and `parse_status` says
/// why when it is false.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewReport {
    pub file_path: String,
    /// `Clean`, `Partial`, `Fallback` or `Failed`, from the buffer's parse.
    pub parse_status: String,
    /// False when the buffer did not parse well enough to diff against.
    pub delta_available: bool,
    /// Whether the file is in the latest generation. Governs whether the
    /// caller graph below has anything to say about it; the symbol delta does
    /// not come from the index.
    pub file_is_indexed: bool,
    /// What the buffer was diffed against: `disk` (the file's current content)
    /// or `nothing` (no such file, so every symbol is an addition).
    pub compared_against: String,
    /// Set when the delta is reported but should be read with care — a partial
    /// parse can hide a symbol and make it look removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub symbols: Vec<PreviewSymbol>,
    /// Symbols present in both versions with an identical declaration, whose
    /// *bodies* nothing compared — one side or the other carries no body
    /// signature, because the body is under the size floor or the file has no
    /// linked grammar.
    ///
    /// A count rather than a row each: on a real file most symbols are small,
    /// and a line per uncompared accessor would bury the handful of findings
    /// the report exists for. But it is not nothing, either — these symbols
    /// were not found to be unchanged, they were not examined — so the number
    /// is carried instead of dropped.
    #[serde(default)]
    pub bodies_not_compared: usize,
    /// Call edges into the affected symbols that fell below the confidence
    /// floor, and so are not listed above.
    ///
    /// Almost always name-only attribution — the resolver matching a bare
    /// `.get(...)` to every `get` in the tree. Counted so that "no callers
    /// affected" cannot quietly mean "none we were willing to vouch for".
    #[serde(default)]
    pub ambiguous_callers: usize,
    /// Calls from other files into symbols this edit removes or re-declares.
    pub broken_callers: Response<PreviewCaller>,
}

/// A duplicate-code report, with the coverage it was computed over.
///
/// The coverage is not a footnote. `groups: []` on its own is unreadable: it
/// says the same thing for a tree with no duplication and for a tree where
/// nothing could be signed, and those call for opposite reactions. Carrying the
/// denominator makes the difference visible without the reader having to know
/// how signing works.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneReport {
    pub groups: Response<devmap_analyze::CloneGroup>,
    /// Symbols that carried a body signature.
    pub signed_symbols: usize,
    /// Symbols with none: below the size floor, a kind with no comparable body,
    /// or a file no grammar parsed.
    pub unsigned_symbols: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolHit {
    pub symbol_name: String,
    pub file_path: String,
    pub kind: String,
    pub span: (u32, u32),
    pub source_span: String, // R2: verbatim source lines grouped by file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_unavailable_reason: Option<String>,
    /// Bytes of the symbol's source that were **not** included in
    /// `source_span`, when it had to be capped to fit the token budget.
    ///
    /// `None` means `source_span` is the whole symbol, which is what R2's
    /// "verbatim" promises. A capped span that said nothing would break that
    /// promise silently — a consumer would read a truncated function body as
    /// the complete one — so the omission is reported rather than implied.
    /// Serialized only when it happened, so existing consumers see no new key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_span_omitted_bytes: Option<u32>,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub subsystems: Vec<SubsystemEntry>,
    pub entry_roots: Vec<String>,
    pub important_files: Vec<String>,
    pub freshness: FreshnessInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsystemEntry {
    pub name: String,
    pub path: String,
    pub entry_points: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessInfo {
    pub head_sha: String,
    pub generation_id: u32,
    pub pending_count: usize,
    /// Freshness fields the *caller* computed, stamped verbatim into both
    /// artifacts when present.
    ///
    /// The kernel cannot compute these — they are SHA-1 digests over the git
    /// file set and no hashing crate is linked in this workspace — and it must
    /// not invent them, because an empty `indexed_hash` that reads as a
    /// computed answer is precisely the confusion `meta.devmap_rust.unavailable`
    /// exists to prevent. So each is `None` until a caller supplies a real
    /// value, and the corresponding "unavailable" marker is emitted only while
    /// it stays `None`.
    ///
    /// This lives on `FreshnessInfo` rather than being passed beside it so that
    /// the map and the graph cannot be stamped from different sources: one
    /// freshness identity, written once, for both artifacts.
    #[serde(default)]
    pub stamped: StampedFreshness,
}

/// Freshness digests supplied by the caller, each independently optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StampedFreshness {
    pub generated_head: Option<String>,
    pub indexed_hash: Option<String>,
    pub content_fingerprint: Option<String>,
}

impl StampedFreshness {
    /// Whether any digest was supplied. Used to decide whether the artifact is
    /// carrying caller-computed freshness at all.
    pub fn is_empty(&self) -> bool {
        self.generated_head.is_none()
            && self.indexed_hash.is_none()
            && self.content_fingerprint.is_none()
    }
}

impl FreshnessInfo {
    /// Construct with no caller-supplied digests — the kernel-only identity.
    pub fn new(head_sha: String, generation_id: u32, pending_count: usize) -> Self {
        Self {
            head_sha,
            generation_id,
            pending_count,
            stamped: StampedFreshness::default(),
        }
    }

    /// `generated_head` as it should appear in an artifact: the caller's value
    /// when it supplied one, otherwise the newest persisted generation's head.
    ///
    /// These answer different questions and the difference is load-bearing.
    /// The kernel's `head_sha` is the head of the last generation actually
    /// persisted; a consumer's staleness check asks whether the artifact
    /// describes the tree at the *current* `git rev-parse HEAD`. An incremental
    /// build that finds nothing changed persists no generation, so after a
    /// commit touching nothing indexed the kernel value points at the previous
    /// commit and the map reads stale the moment it is written.
    pub fn generated_head(&self) -> &str {
        self.stamped
            .generated_head
            .as_deref()
            .unwrap_or(&self.head_sha)
    }
}
