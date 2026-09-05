use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadSymbolReport {
    pub symbol_name: String,
    pub file_path: String,
    pub confidence: f32,
    pub is_exempt: bool,
    pub exemption_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityReport {
    pub community_id: u32,
    pub name: String,
    pub members: Vec<String>,
    pub cohesion_score: f32, // R6: every community reports cohesion
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSummary {
    pub total_files: usize,
    pub total_symbols: usize,
    pub total_edges: usize,
    pub dead_symbols: Vec<DeadSymbolReport>,
    pub communities: Vec<CommunityReport>,
    pub status: AnalysisStatus,
    /// Calls the resolution ladder could not attribute (R5 / D17).
    ///
    /// Persisted with the generation so a reader can tell "nothing calls this"
    /// apart from "we could not work out what this calls". `serde(default)`
    /// keeps pre-existing serialized summaries readable, where the field's
    /// absence honestly means "not recorded", not "zero".
    #[serde(default)]
    pub unresolved_calls: usize,
    /// How much of this generation carries a body signature.
    ///
    /// Not the duplicate groups themselves — those are derived on demand from
    /// the signature columns on the symbol table, so persisting them here would
    /// be a truncated second copy of a derivable fact. What is *not* derivable
    /// is how many symbols were never signed, which is the denominator every
    /// clone report has to be read against.
    ///
    /// `serde(default)` yields zeroes for generations written before clone
    /// detection existed, and zero signed symbols is exactly the truth about
    /// them: nothing was examined.
    #[serde(default)]
    pub clone_coverage: crate::clones::CloneCoverage,
    /// How many files discovery refused to read when this generation was built.
    ///
    /// Persisted because it cannot be recovered from the stored graph: a file
    /// discovery turned away has no rows at all, so a later reader cannot count
    /// what is missing by looking at what is there.
    ///
    /// The daemon is why this matters. Its incremental resync carries the
    /// previous generation's extractions forward and never re-walks discovery,
    /// so it cannot measure refusals itself — and it overwrites this summary on
    /// every drain. Without the count to carry forward, the `Partial` that
    /// `devmap build` correctly recorded survived only until the next watcher
    /// event, and a corpus with unread files was relabelled complete.
    ///
    /// `None` means **not recorded** — no discovery step's result was reported
    /// — and is not the same as `Some(0)`, which is a measurement that found
    /// nothing refused. `serde(default)` yields `None` for generations written
    /// before this field existed, which is the honest reading of them.
    #[serde(default)]
    pub discovery_refused_files: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnalysisStatus {
    Ok,
    Partial { reason: String },
    Timeout { reason: String },
}
