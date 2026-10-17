//! The labels file, in exactly the shape `golabel` writes and
//! `real_corpus_accuracy.rs` reads (`rust/testdata/accuracy/README.md`,
//! "Labels file shape"): `recall`, `oracle` and `dead`.

use serde::Serialize;

/// One call or use site and the in-repo callee rust-analyzer bound it to; the
/// scorer counts it present when DevMap has a use edge from `caller` to any id
/// in `truth` (`a|b`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecallLabel {
    pub language: String,
    pub construct: String,
    pub caller: String,
    pub name: String,
    pub truth: String,
}

/// One `(caller, callee)` pair the oracle bound. `impls` lists the in-repo
/// implementations of `callee` when it is a trait method declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Use {
    pub caller: String,
    pub callee: String,
    pub impls: Vec<String>,
}

/// What the oracle read, and every use it bound from the callers it read in
/// full. A caller with any site the oracle could not answer is not in
/// `covered_callers`, so DevMap's edges from it are left unjudged rather than
/// judged wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Oracle {
    pub language: String,
    pub covered_callers: Vec<String>,
    pub uses: Vec<Use>,
}

/// A hand-labelled dead row, merged verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct DeadLabel {
    pub language: String,
    pub id: String,
    pub truly_dead: bool,
}

/// The labels file the corpus runner reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Labels {
    pub recall: Vec<RecallLabel>,
    pub oracle: Vec<Oracle>,
    pub dead: Vec<DeadLabel>,
}

impl Labels {
    /// Indented JSON with a trailing newline: the same bytes for the same
    /// corpus.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}
