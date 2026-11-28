// Stateful testing and fuzzing foundation for DevMap.
//
// Included into test crates and standalone fuzz targets via:
//
//     #[allow(dead_code)]
//     mod stateful_fuzz {
//         include!("../../testsupport/stateful_fuzz.rs");
//     }
//
// Design constraints:
// - `std` only (zero external crate dependencies, zero `unsafe`).
// - No inner doc comments (`//!`) or inner attributes (`#![...]`) so `include!`
//   works inside any module or test file.
// - Dual-driver `Source` supports deterministic seeded `cargo test` runs and
//   raw byte-backed `cargo-fuzz` / libFuzzer targets through the same API.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

/// Read a `u64` from environment variable `key`, falling back to `default` when
/// unset or empty. Panics on a malformed non-empty value so typos fail loudly.
pub fn env_u64(key: &str, default: u64) -> u64 {
    match std::env::var(key) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return default;
            }
            if let Some(hex) = trimmed
                .strip_prefix("0x")
                .or_else(|| trimmed.strip_prefix("0X"))
            {
                u64::from_str_radix(hex, 16)
                    .unwrap_or_else(|_| panic!("{key}={raw:?} is not a valid hex u64"))
            } else {
                trimmed
                    .parse::<u64>()
                    .unwrap_or_else(|_| panic!("{key}={raw:?} is not a valid u64"))
            }
        }
        Err(_) => default,
    }
}

/// Read a `usize` from environment variable `key`, falling back to `default`
/// when unset or empty. Panics on a malformed non-empty value.
pub fn env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return default;
            }
            trimmed
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{key}={raw:?} is not a valid usize"))
        }
        Err(_) => default,
    }
}

// ---------------------------------------------------------------------------
// Dual-driver Source (deterministic SplitMix64 OR raw libFuzzer byte slice)
// ---------------------------------------------------------------------------

/// Choice stream driving stateful actions and generators.
///
/// Backed either by a deterministic SplitMix64 PRNG (`Source::from_seed`) for
/// fast `cargo test` campaigns, or by a raw byte buffer (`Source::from_bytes`)
/// for coverage-guided `cargo-fuzz` / libFuzzer targets. Once a byte-backed
/// stream is exhausted, reads gracefully return `0` / `false` / empty slices.
#[derive(Clone, Debug)]
pub struct Source {
    seed: u64,
    state: u64,
    bytes: Option<Vec<u8>>,
    pos: usize,
    more_run: usize,
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl Source {
    /// Deterministic PRNG-backed source using SplitMix64.
    pub fn from_seed(seed: u64) -> Self {
        Self {
            seed,
            state: seed,
            bytes: None,
            pos: 0,
            more_run: 0,
        }
    }

    /// Raw byte-backed source for `cargo-fuzz` / libFuzzer targets.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let seed = if bytes.is_empty() { 0 } else { fnv1a_64(bytes) };
        Self {
            seed,
            state: seed,
            bytes: Some(bytes.to_vec()),
            pos: 0,
            more_run: 0,
        }
    }

    /// Seed that initialized this source (or FNV-1a digest of raw input bytes).
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// True when backed by a finite byte buffer rather than a PRNG stream.
    pub fn is_byte_backed(&self) -> bool {
        self.bytes.is_some()
    }

    /// Remaining unread bytes in byte-backed mode, or `0` in seeded mode.
    pub fn remaining_bytes(&self) -> usize {
        self.bytes
            .as_ref()
            .map_or(0, |buf| buf.len().saturating_sub(self.pos))
    }

    fn next_splitmix64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Draw one byte (`0` once a byte-backed stream is exhausted).
    pub fn u8(&mut self) -> u8 {
        if let Some(buf) = &self.bytes {
            if self.pos < buf.len() {
                let b = buf[self.pos];
                self.pos += 1;
                return b;
            }
            return 0;
        }
        (self.next_splitmix64() >> 56) as u8
    }

    /// Draw a little-endian `u32` (zero-padded once byte-backed stream ends).
    pub fn u32(&mut self) -> u32 {
        if self.bytes.is_some() {
            let mut raw = [0u8; 4];
            for slot in &mut raw {
                *slot = self.u8();
            }
            return u32::from_le_bytes(raw);
        }
        (self.next_splitmix64() >> 32) as u32
    }

    /// Draw a little-endian `u64` (zero-padded once byte-backed stream ends).
    pub fn u64(&mut self) -> u64 {
        if self.bytes.is_some() {
            let mut raw = [0u8; 8];
            for slot in &mut raw {
                *slot = self.u8();
            }
            return u64::from_le_bytes(raw);
        }
        self.next_splitmix64()
    }

    /// Draw an index in `0..bound` (returns `0` when `bound <= 1`).
    pub fn usize_below(&mut self, bound: usize) -> usize {
        if bound <= 1 {
            return 0;
        }
        if self.bytes.is_some() {
            let raw = if bound <= 256 {
                usize::from(self.u8())
            } else if bound <= 65_536 {
                let lo = usize::from(self.u8());
                let hi = usize::from(self.u8());
                lo | (hi << 8)
            } else {
                self.u64() as usize
            };
            return raw % bound;
        }
        (self.next_splitmix64() as usize) % bound
    }

    /// Draw a boolean (`false` once a byte-backed stream is exhausted).
    pub fn bool(&mut self) -> bool {
        (self.u8() & 1) == 1
    }

    /// Return `true` with probability `num / den` (`false` when exhausted).
    pub fn chance(&mut self, num: u32, den: u32) -> bool {
        if den == 0 || num == 0 {
            return false;
        }
        if self.is_byte_backed() && self.remaining_bytes() == 0 {
            return false;
        }
        if num >= den {
            return true;
        }
        let draw = if self.is_byte_backed() && den <= 256 {
            u32::from(self.u8()) % den
        } else {
            self.u32() % den
        };
        draw < num
    }

    /// Pick one element from `slice`, or `None` if empty.
    pub fn choose<'a, T>(&mut self, slice: &'a [T]) -> Option<&'a T> {
        if slice.is_empty() {
            return None;
        }
        let idx = self.usize_below(slice.len());
        Some(&slice[idx])
    }

    /// Draw a byte vector of length `0..=max_len`.
    pub fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        if max_len == 0 {
            return Vec::new();
        }
        if self.is_byte_backed() && self.remaining_bytes() == 0 {
            return Vec::new();
        }
        let mut len = self.usize_below(max_len.saturating_add(1));
        if let Some(buf) = &self.bytes {
            let avail = buf.len().saturating_sub(self.pos);
            len = len.min(avail);
            let out = buf[self.pos..self.pos + len].to_vec();
            self.pos += len;
            return out;
        }
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.u8());
        }
        out
    }

    /// Generate a valid identifier prefixed with `prefix` (safe across Rust,
    /// TypeScript, Go, and Python even when `prefix` is a keyword like `"fn"`).
    pub fn ident(&mut self, prefix: &str) -> String {
        let n = self.usize_below(10_000);
        if prefix.is_empty() {
            format!("id_{n}")
        } else {
            format!("{prefix}_{n}")
        }
    }

    /// Bounded continuation loop condition (`s.more()`) capped at `max_items`.
    ///
    /// Returns `false` once `max_items` consecutive items have been yielded,
    /// when `max_items == 0`, or when a byte-backed stream is exhausted.
    pub fn more(&mut self, max_items: usize) -> bool {
        if max_items == 0 || self.more_run >= max_items {
            self.more_run = 0;
            return false;
        }
        if self.is_byte_backed() && self.remaining_bytes() == 0 {
            self.more_run = 0;
            return false;
        }
        // Continue with probability ~75%, resetting on stop so subsequent loops
        // start fresh.
        if self.chance(3, 4) {
            self.more_run += 1;
            true
        } else {
            self.more_run = 0;
            false
        }
    }

    /// Derive a deterministic child seed for sub-generators or nested steps.
    pub fn fork_seed(&mut self) -> u64 {
        let raw = self.u64();
        raw.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ self.seed.rotate_left(17)
    }
}

// ---------------------------------------------------------------------------
// `require` / `bail` vs `check` semantics
// ---------------------------------------------------------------------------

/// A violated semantic oracle (real bug candidate, distinct from a `Bail`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckFailure {
    pub oracle: &'static str,
    pub action: &'static str,
    pub detail: String,
}

impl CheckFailure {
    /// Construct a `CheckFailure`.
    pub fn new(oracle: &'static str, action: &'static str, detail: impl Into<String>) -> Self {
        Self {
            oracle,
            action,
            detail: detail.into(),
        }
    }

    /// Stable deduplication / minimization signature (`oracle|action`).
    pub fn signature(&self) -> String {
        format!("{}|{}", self.oracle, self.action)
    }
}

impl fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{} @ {}] {}", self.oracle, self.action, self.detail)
    }
}

/// Outcome of executing one stateful action step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// Action ran and all oracles held.
    Ok,
    /// Expected precondition/input rejection (not a bug).
    Bail(&'static str),
    /// Action could not run because a required object/state was missing.
    SkipMissing(&'static str),
    /// Semantic oracle violation (bug!).
    CheckFailed(CheckFailure),
}

impl StepOutcome {
    /// True if the step succeeded (`StepOutcome::Ok`).
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }

    /// Borrow the `CheckFailure` if this step failed an oracle.
    pub fn failure(&self) -> Option<&CheckFailure> {
        match self {
            Self::CheckFailed(f) => Some(f),
            _ => None,
        }
    }

    /// Convert into `Some(CheckFailure)` if this step failed an oracle.
    pub fn into_failure(self) -> Option<CheckFailure> {
        match self {
            Self::CheckFailed(f) => Some(f),
            _ => None,
        }
    }
}

/// Construct an expected precondition/input rejection (`StepOutcome::Bail`).
pub fn bail(reason: &'static str) -> StepOutcome {
    StepOutcome::Bail(reason)
}

/// Construct a skipped-missing-input outcome (`StepOutcome::SkipMissing`).
pub fn skip_missing(reason: &'static str) -> StepOutcome {
    StepOutcome::SkipMissing(reason)
}

/// Assert a semantic oracle invariant; returns `Ok` when `cond` holds, or
/// `StepOutcome::CheckFailed` otherwise.
pub fn check(
    cond: bool,
    oracle: &'static str,
    action: &'static str,
    detail: impl Into<String>,
) -> StepOutcome {
    if cond {
        StepOutcome::Ok
    } else {
        StepOutcome::CheckFailed(CheckFailure::new(oracle, action, detail))
    }
}

// ---------------------------------------------------------------------------
// Online Health Bandit & `probe` Telemetry
// ---------------------------------------------------------------------------

/// Per-action execution counters tracked by `HealthTracker`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionStats {
    pub invoked: usize,
    pub ok: usize,
    pub bail: usize,
    pub skip_missing: usize,
    pub failed: usize,
}

/// Online harness-health tracker and producer-pair bandit with `probe`
/// telemetry.
#[derive(Clone, Debug, Default)]
pub struct HealthTracker {
    actions: BTreeMap<&'static str, ActionStats>,
    bail_reasons: BTreeMap<&'static str, usize>,
    skip_reasons: BTreeMap<&'static str, usize>,
    /// `(consumer_action, prev_action) -> (calls, ok)`
    pairs: BTreeMap<(&'static str, &'static str), (usize, usize)>,
}

impl HealthTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the outcome of running `action` after optional `prev` action.
    pub fn record(
        &mut self,
        action: &'static str,
        prev: Option<&'static str>,
        outcome: &StepOutcome,
    ) {
        let entry = self.actions.entry(action).or_default();
        match outcome {
            StepOutcome::Ok => {
                entry.invoked += 1;
                entry.ok += 1;
                if let Some(p) = prev {
                    let pair = self.pairs.entry((action, p)).or_insert((0, 0));
                    pair.0 += 1;
                    pair.1 += 1;
                }
            }
            StepOutcome::Bail(reason) => {
                entry.invoked += 1;
                entry.bail += 1;
                *self.bail_reasons.entry(reason).or_insert(0) += 1;
                if let Some(p) = prev {
                    let pair = self.pairs.entry((action, p)).or_insert((0, 0));
                    pair.0 += 1;
                }
            }
            StepOutcome::SkipMissing(reason) => {
                entry.skip_missing += 1;
                *self.skip_reasons.entry(reason).or_insert(0) += 1;
            }
            StepOutcome::CheckFailed(_) => {
                entry.invoked += 1;
                entry.failed += 1;
                if let Some(p) = prev {
                    let pair = self.pairs.entry((action, p)).or_insert((0, 0));
                    pair.0 += 1;
                }
            }
        }
    }

    /// Selection weight for `action`.
    ///
    /// Actions that never succeed after 200 invocations are throttled to `0.05`
    /// (retried occasionally); rarely-succeeding actions get an exploration
    /// bonus up to `3.0`.
    pub fn ep_weight(&self, action: &str) -> f64 {
        let s = self.actions.get(action).copied().unwrap_or_default();
        if s.ok == 0 && s.invoked >= 200 {
            return 0.05;
        }
        if s.ok == 0 && s.invoked >= 40 {
            return 0.5;
        }
        1.0 + 2.0 / (1.0 + (s.ok as f64) / 64.0)
    }

    /// Laplace-smoothed success rate `(ok + 1) / (calls + 2)` of calling
    /// `consumer` after `prev` (unseen prior `0.6`, floor `0.02`).
    pub fn pair_rate(&self, consumer: &str, prev: &str) -> f64 {
        match self.pairs.get(&(consumer, prev)) {
            Some(&(calls, ok)) => ((ok as f64 + 1.0) / (calls as f64 + 2.0)).max(0.02),
            None => 0.6,
        }
    }

    /// Pick one candidate action weighted by `ep_weight * pair_rate`.
    pub fn pick_weighted(
        &self,
        src: &mut Source,
        candidates: &[&'static str],
        prev: Option<&'static str>,
    ) -> &'static str {
        assert!(
            !candidates.is_empty(),
            "HealthTracker::pick_weighted called with empty candidates"
        );
        if candidates.len() == 1 {
            return candidates[0];
        }
        let mut weights = Vec::with_capacity(candidates.len());
        let mut total = 0.0f64;
        for &cand in candidates {
            let mut w = self.ep_weight(cand);
            if let Some(p) = prev {
                w *= self.pair_rate(cand, p);
            }
            let w = w.max(0.0);
            weights.push(w);
            total += w;
        }
        if total <= 0.0 {
            let idx = src.usize_below(candidates.len());
            return candidates[idx];
        }
        let unit = f64::from(src.u32()) / (f64::from(u32::MAX) + 1.0);
        let mut target = unit * total;
        for (i, &w) in weights.iter().enumerate() {
            if target < w {
                return candidates[i];
            }
            target -= w;
        }
        candidates[candidates.len() - 1]
    }

    /// Total invoked actions (`Ok` + `Bail` + `CheckFailed`).
    pub fn total_invoked(&self) -> usize {
        self.actions.values().map(|s| s.invoked).sum()
    }

    /// Total `Ok` outcomes across all actions.
    pub fn total_ok(&self) -> usize {
        self.actions.values().map(|s| s.ok).sum()
    }

    /// Total `Bail` outcomes across all actions.
    pub fn total_bails(&self) -> usize {
        self.actions.values().map(|s| s.bail).sum()
    }

    /// Total `CheckFailed` outcomes across all actions.
    pub fn total_failed(&self) -> usize {
        self.actions.values().map(|s| s.failed).sum()
    }

    /// Per-action counters (`ActionStats::default()` if never seen).
    pub fn action_stats(&self, action: &str) -> ActionStats {
        self.actions.get(action).copied().unwrap_or_default()
    }

    /// Bail-reason histogram.
    pub fn bail_reasons(&self) -> &BTreeMap<&'static str, usize> {
        &self.bail_reasons
    }

    /// Format a harness-health probe report for test diagnostics.
    pub fn probe_report(&self, suite: &str) -> String {
        let invoked = self.total_invoked();
        let ok = self.total_ok();
        let bails = self.total_bails();
        let failed = self.total_failed();
        let bail_pct = if invoked > 0 {
            (bails as f64) * 100.0 / (invoked as f64)
        } else {
            0.0
        };

        let mut out = String::new();
        let _ = writeln!(
            out,
            "=== probe: {suite} (invoked={invoked}, ok={ok}, bails={bails} [{bail_pct:.1}%], failed={failed}) ==="
        );
        let _ = writeln!(
            out,
            "{:<28} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
            "action", "invoked", "ok", "bail", "skip", "fail", "bail%"
        );
        let mut never_ok = Vec::new();
        for (&name, s) in &self.actions {
            let pct = if s.invoked > 0 {
                (s.bail as f64) * 100.0 / (s.invoked as f64)
            } else {
                0.0
            };
            let _ = writeln!(
                out,
                "{name:<28} {:>7} {:>7} {:>7} {:>7} {:>7} {:>6.1}%",
                s.invoked, s.ok, s.bail, s.skip_missing, s.failed, pct
            );
            if s.ok == 0 {
                never_ok.push(name);
            }
        }
        if !never_ok.is_empty() {
            let _ = writeln!(out, "never_succeeded: {}", never_ok.join(", "));
        }
        if !self.bail_reasons.is_empty() {
            let mut reasons: Vec<(&&'static str, &usize)> = self.bail_reasons.iter().collect();
            reasons.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let top: Vec<String> = reasons
                .into_iter()
                .take(8)
                .map(|(r, c)| format!("{r:?} x{c}"))
                .collect();
            let _ = writeln!(out, "top_bail_reasons: {}", top.join(", "));
        }
        if !self.skip_reasons.is_empty() {
            let mut reasons: Vec<(&&'static str, &usize)> = self.skip_reasons.iter().collect();
            reasons.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let top: Vec<String> = reasons
                .into_iter()
                .take(6)
                .map(|(r, c)| format!("{r:?} x{c}"))
                .collect();
            let _ = writeln!(out, "top_skip_reasons: {}", top.join(", "));
        }
        out
    }

    /// Panic with `probe_report` if any action in `required_ok` achieved zero
    /// `Ok` outcomes (prevents vacuous fuzzers that only bail or skip).
    pub fn assert_actions_exercised(&self, required_ok: &[&'static str]) {
        let missing: Vec<&'static str> = required_ok
            .iter()
            .copied()
            .filter(|&name| self.action_stats(name).ok == 0)
            .collect();
        if !missing.is_empty() {
            panic!(
                "probe failed: required actions never succeeded: {missing:?}\n{}",
                self.probe_report("assert_actions_exercised")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Greedy Call-Sequence & Payload Minimizer + `#[test]` Emitter
// ---------------------------------------------------------------------------

/// Shrink `actions` while preserving a failure matching `expected_sig`.
///
/// Strategy:
/// 1. Coarse prefix/suffix half drops.
/// 2. Chunked delta-debugging (`chunk = len/2, len/4, ..., 1`).
/// 3. Fixed-point last-first single-action drops.
pub fn minimize_sequence<A: Clone, F>(
    actions: &[A],
    expected_sig: &str,
    max_attempts: usize,
    mut run: F,
) -> Vec<A>
where
    F: FnMut(&[A]) -> Option<CheckFailure>,
{
    let mut cur = actions.to_vec();
    if cur.len() <= 1 || max_attempts == 0 {
        return cur;
    }
    let mut attempts = 0usize;
    let mut reproduces = |cand: &[A], attempts: &mut usize| -> bool {
        if *attempts >= max_attempts {
            return false;
        }
        *attempts += 1;
        matches!(run(cand), Some(ref f) if f.signature() == expected_sig)
    };

    // 1. Coarse prefix / suffix halving.
    while cur.len() > 1 && attempts < max_attempts {
        let mid = cur.len() / 2;
        if reproduces(&cur[..mid], &mut attempts) {
            cur.truncate(mid);
            continue;
        }
        if reproduces(&cur[mid..], &mut attempts) {
            cur = cur[mid..].to_vec();
            continue;
        }
        break;
    }

    // 2. Delta-debugging chunks + last-first single-action drops until fixed point.
    let mut changed = true;
    while changed && cur.len() > 1 && attempts < max_attempts {
        changed = false;

        let mut chunk = cur.len() / 2;
        while chunk >= 2 && cur.len() > 1 && attempts < max_attempts {
            let mut at = 0usize;
            while at < cur.len() && cur.len() > 1 && attempts < max_attempts {
                let end = (at + chunk).min(cur.len());
                if end - at == cur.len() {
                    break;
                }
                let mut cand = Vec::with_capacity(cur.len() - (end - at));
                cand.extend_from_slice(&cur[..at]);
                cand.extend_from_slice(&cur[end..]);
                if !cand.is_empty() && reproduces(&cand, &mut attempts) {
                    cur = cand;
                    changed = true;
                } else {
                    at += chunk;
                }
            }
            chunk /= 2;
        }

        let mut idx = cur.len();
        while idx > 0 && cur.len() > 1 && attempts < max_attempts {
            idx -= 1;
            if idx >= cur.len() {
                continue;
            }
            let mut cand = Vec::with_capacity(cur.len() - 1);
            cand.extend_from_slice(&cur[..idx]);
            cand.extend_from_slice(&cur[idx + 1..]);
            if !cand.is_empty() && reproduces(&cand, &mut attempts) {
                cur = cand;
                changed = true;
            }
        }
    }

    cur
}

fn prev_char_boundary(s: &str, mut byte_idx: usize) -> usize {
    if byte_idx >= s.len() {
        return s.len();
    }
    while byte_idx > 0 && !s.is_char_boundary(byte_idx) {
        byte_idx -= 1;
    }
    byte_idx
}

/// Shrink `input` while preserving UTF-8 char boundaries and `expected_sig`.
///
/// Tries: empty string -> prefix/suffix halves -> line drops -> char-indexed
/// chunk delta-debugging.
pub fn minimize_str<F>(input: &str, expected_sig: &str, max_attempts: usize, mut run: F) -> String
where
    F: FnMut(&str) -> Option<CheckFailure>,
{
    let mut cur = input.to_string();
    if cur.is_empty() || max_attempts == 0 {
        return cur;
    }
    let mut attempts = 0usize;
    let mut reproduces = |cand: &str, attempts: &mut usize| -> bool {
        if *attempts >= max_attempts {
            return false;
        }
        *attempts += 1;
        matches!(run(cand), Some(ref f) if f.signature() == expected_sig)
    };

    // 1. Empty string.
    if reproduces("", &mut attempts) {
        return String::new();
    }

    // 2. Coarse prefix / suffix halves at char boundaries.
    while cur.len() > 1 && attempts < max_attempts {
        let mid = prev_char_boundary(&cur, cur.len() / 2);
        if mid > 0 && mid < cur.len() {
            if reproduces(&cur[..mid], &mut attempts) {
                cur.truncate(mid);
                continue;
            }
            if reproduces(&cur[mid..], &mut attempts) {
                cur = cur[mid..].to_string();
                continue;
            }
        }
        break;
    }

    // 3. Line-level drops (last first).
    if cur.contains('\n') && attempts < max_attempts {
        let mut lines: Vec<String> = cur.split_inclusive('\n').map(String::from).collect();
        let mut changed = true;
        while changed && lines.len() > 1 && attempts < max_attempts {
            changed = false;
            let mut idx = lines.len();
            while idx > 0 && lines.len() > 1 && attempts < max_attempts {
                idx -= 1;
                if idx >= lines.len() {
                    continue;
                }
                let mut cand = String::with_capacity(cur.len());
                for (i, line) in lines.iter().enumerate() {
                    if i != idx {
                        cand.push_str(line);
                    }
                }
                if reproduces(&cand, &mut attempts) {
                    lines.remove(idx);
                    cur = cand;
                    changed = true;
                }
            }
        }
    }

    // 4. Char-indexed chunk delta-debugging (`chunk = chars/2 .. 1`).
    let mut changed = true;
    while changed && attempts < max_attempts {
        changed = false;
        let char_count = cur.chars().count();
        if char_count <= 1 {
            break;
        }
        let mut chunk = (char_count / 2).max(1);
        while chunk >= 1 && attempts < max_attempts {
            let mut char_pos = 0usize;
            loop {
                let offsets: Vec<usize> = cur.char_indices().map(|(o, _)| o).collect();
                if char_pos >= offsets.len() || offsets.len() <= 1 || attempts >= max_attempts {
                    break;
                }
                let end_char = (char_pos + chunk).min(offsets.len());
                let start_byte = offsets[char_pos];
                let end_byte = if end_char < offsets.len() {
                    offsets[end_char]
                } else {
                    cur.len()
                };
                if start_byte == 0 && end_byte == cur.len() {
                    break;
                }
                let mut cand = String::with_capacity(cur.len() - (end_byte - start_byte));
                cand.push_str(&cur[..start_byte]);
                cand.push_str(&cur[end_byte..]);
                if reproduces(&cand, &mut attempts) {
                    cur = cand;
                    changed = true;
                } else {
                    char_pos += chunk;
                }
            }
            chunk /= 2;
        }
    }

    cur
}

/// Emit a copy-pasteable `#[test]` reproducer stub with replay env hint and
/// normalized failure signature.
pub fn format_repro_test(
    test_name: &str,
    seed: u64,
    failure: &CheckFailure,
    actions: &[String],
) -> String {
    let fn_name: String = test_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fn_name = if fn_name.is_empty() || fn_name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("repro_{fn_name}")
    } else {
        fn_name
    };

    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Replay: DEVMAP_STATEFUL_SEED={seed} cargo test {fn_name}"
    );
    let _ = writeln!(out, "// Signature: {}", failure.signature());
    let _ = writeln!(out, "// Failure: {failure}");
    let _ = writeln!(out, "#[test]");
    let _ = writeln!(out, "fn {fn_name}() {{");
    let _ = writeln!(out, "    let actions: &[&str] = &[");
    for (i, act) in actions.iter().enumerate() {
        let _ = writeln!(out, "        // step {i}");
        let _ = writeln!(out, "        {act:?},");
    }
    let _ = writeln!(out, "    ];");
    let _ = writeln!(
        out,
        "    panic!(\"repro captured {} step(s): {{:?}}\", actions);",
        actions.len()
    );
    let _ = writeln!(out, "}}");
    out
}

// ---------------------------------------------------------------------------
// Structure-Aware Multi-Language Micro-Repo Generator
// ---------------------------------------------------------------------------

/// Languages supported by the structure-aware micro-repo generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MicroLang {
    Rust,
    TypeScript,
    Go,
    Python,
}

impl MicroLang {
    /// Canonical file extension (without leading dot).
    pub fn ext(&self) -> &'static str {
        match self {
            Self::Rust => "rs",
            Self::TypeScript => "ts",
            Self::Go => "go",
            Self::Python => "py",
        }
    }
}

/// Virtual multi-file repository mapping repo-relative paths to UTF-8 sources.
pub type MicroRepo = BTreeMap<String, String>;

/// Generate a valid, compilable-shaped multi-file micro-repo across `langs`
/// with cross-file imports, callers, callees, structs/classes, and dead helpers.
pub fn generate_micro_repo(src: &mut Source, langs: &[MicroLang], num_files: usize) -> MicroRepo {
    let mut repo = MicroRepo::new();
    if langs.is_empty() || num_files == 0 {
        return repo;
    }

    let mut per_lang_idx: BTreeMap<MicroLang, usize> = BTreeMap::new();
    for slot in 0..num_files {
        let lang = langs[slot % langs.len()];
        let idx = *per_lang_idx.get(&lang).unwrap_or(&0);
        per_lang_idx.insert(lang, idx + 1);
        let (path, content) = gen_lang_file(src, lang, idx);
        repo.insert(path, content);
    }

    repo
}

fn gen_lang_file(src: &mut Source, lang: MicroLang, idx: usize) -> (String, String) {
    let prev = if idx > 0 {
        Some(src.usize_below(idx))
    } else {
        None
    };
    let k = (src.usize_below(97) + 1) as i32;
    let extra_calls = src.usize_below(3);

    match lang {
        MicroLang::Rust => {
            let path = format!("src/mod_{idx}.rs");
            let mut out = String::new();
            if let Some(p) = prev {
                let _ = writeln!(out, "use crate::mod_{p}::api_{p};\n");
            }
            let _ = writeln!(out, "pub struct Widget{idx} {{");
            let _ = writeln!(out, "    pub value: i32,");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "impl Widget{idx} {{");
            let _ = writeln!(out, "    pub fn step(&self, x: i32) -> i32 {{");
            let _ = writeln!(out, "        helper_{idx}(self.value + x)");
            let _ = writeln!(out, "    }}");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "pub fn api_{idx}(x: i32) -> i32 {{");
            if let Some(p) = prev {
                let _ = writeln!(out, "    let base = api_{p}(x);");
            } else {
                let _ = writeln!(out, "    let base = x + {k};");
            }
            let _ = writeln!(out, "    let w = Widget{idx} {{ value: base }};");
            let _ = writeln!(out, "    w.step(helper_{idx}(base))");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "pub fn helper_{idx}(x: i32) -> i32 {{");
            let _ = writeln!(out, "    x + {k}");
            let _ = writeln!(out, "}}\n");
            for e in 0..extra_calls {
                let _ = writeln!(out, "pub fn extra_{idx}_{e}(x: i32) -> i32 {{");
                let _ = writeln!(out, "    helper_{idx}(x) + {e}");
                let _ = writeln!(out, "}}\n");
            }
            let _ = writeln!(out, "fn dead_helper_{idx}(x: i32) -> i32 {{");
            let _ = writeln!(out, "    x.wrapping_mul(3)");
            let _ = writeln!(out, "}}");
            (path, out)
        }
        MicroLang::TypeScript => {
            let path = format!("ts/mod_{idx}.ts");
            let mut out = String::new();
            if let Some(p) = prev {
                let _ = writeln!(out, "import {{ api_{p} }} from \"./mod_{p}\";\n");
            }
            let _ = writeln!(out, "export class Service{idx} {{");
            let _ = writeln!(out, "  step(x: number): number {{");
            let _ = writeln!(out, "    return helper_{idx}(x + {k});");
            let _ = writeln!(out, "  }}");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "export function api_{idx}(x: number): number {{");
            if let Some(p) = prev {
                let _ = writeln!(out, "  const base = api_{p}(x);");
            } else {
                let _ = writeln!(out, "  const base = x + {k};");
            }
            let _ = writeln!(out, "  const svc = new Service{idx}();");
            let _ = writeln!(out, "  return svc.step(helper_{idx}(base));");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "export function helper_{idx}(x: number): number {{");
            let _ = writeln!(out, "  return x + {k};");
            let _ = writeln!(out, "}}\n");
            for e in 0..extra_calls {
                let _ = writeln!(out, "export function extra_{idx}_{e}(x: number): number {{");
                let _ = writeln!(out, "  return helper_{idx}(x) + {e};");
                let _ = writeln!(out, "}}\n");
            }
            let _ = writeln!(out, "function deadHelper_{idx}(x: number): number {{");
            let _ = writeln!(out, "  return x * 2;");
            let _ = writeln!(out, "}}");
            (path, out)
        }
        MicroLang::Go => {
            let path = format!("go/mod_{idx}.go");
            let mut out = String::new();
            let _ = writeln!(out, "package micro\n");
            let _ = writeln!(out, "type Worker{idx} struct {{");
            let _ = writeln!(out, "\tValue int");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "func (w *Worker{idx}) Step(x int) int {{");
            let _ = writeln!(out, "\treturn helper{idx}(w.Value + x)");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "func Api{idx}(x int) int {{");
            if let Some(p) = prev {
                let _ = writeln!(out, "\tbase := Api{p}(x)");
            } else {
                let _ = writeln!(out, "\tbase := x + {k}");
            }
            let _ = writeln!(out, "\tw := &Worker{idx}{{Value: base}}");
            let _ = writeln!(out, "\treturn w.Step(helper{idx}(base))");
            let _ = writeln!(out, "}}\n");
            let _ = writeln!(out, "func helper{idx}(x int) int {{");
            let _ = writeln!(out, "\treturn x + {k}");
            let _ = writeln!(out, "}}\n");
            for e in 0..extra_calls {
                let _ = writeln!(out, "func Extra{idx}_{e}(x int) int {{");
                let _ = writeln!(out, "\treturn helper{idx}(x) + {e}");
                let _ = writeln!(out, "}}\n");
            }
            let _ = writeln!(out, "func deadHelper{idx}(x int) int {{");
            let _ = writeln!(out, "\treturn x - 1");
            let _ = writeln!(out, "}}");
            (path, out)
        }
        MicroLang::Python => {
            let path = format!("py/mod_{idx}.py");
            let mut out = String::new();
            if let Some(p) = prev {
                let _ = writeln!(out, "from .mod_{p} import api_{p}\n");
            }
            let _ = writeln!(out, "class Engine{idx}:");
            let _ = writeln!(out, "    def step(self, x: int) -> int:");
            let _ = writeln!(out, "        return helper_{idx}(x + {k})\n");
            let _ = writeln!(out, "def api_{idx}(x: int) -> int:");
            if let Some(p) = prev {
                let _ = writeln!(out, "    base = api_{p}(x)");
            } else {
                let _ = writeln!(out, "    base = x + {k}");
            }
            let _ = writeln!(out, "    eng = Engine{idx}()");
            let _ = writeln!(out, "    return eng.step(helper_{idx}(base))\n");
            let _ = writeln!(out, "def helper_{idx}(x: int) -> int:");
            let _ = writeln!(out, "    return x + {k}\n");
            for e in 0..extra_calls {
                let _ = writeln!(out, "def extra_{idx}_{e}(x: int) -> int:");
                let _ = writeln!(out, "    return helper_{idx}(x) + {e}\n");
            }
            let _ = writeln!(out, "def _dead_helper_{idx}(x: int) -> int:");
            let _ = writeln!(out, "    return x * 3");
            (path, out)
        }
    }
}

fn is_header_block(lang: MicroLang, block: &str) -> bool {
    let trimmed = block.trim_start();
    match lang {
        MicroLang::Rust => trimmed.starts_with("use ") || trimmed.starts_with("mod "),
        MicroLang::TypeScript => trimmed.starts_with("import "),
        MicroLang::Go => trimmed.starts_with("package ") || trimmed.starts_with("import "),
        MicroLang::Python => trimmed.starts_with("from ") || trimmed.starts_with("import "),
    }
}

fn extract_first_fn_name(lang: MicroLang, block: &str) -> Option<String> {
    let keyword = match lang {
        MicroLang::Rust => "fn ",
        MicroLang::TypeScript => "function ",
        MicroLang::Go => "func ",
        MicroLang::Python => "def ",
    };
    for line in block.lines() {
        let trimmed = line.trim();
        if let Some(pos) = trimmed.find(keyword) {
            let after = &trimmed[pos + keyword.len()..];
            // Skip Go method receivers `func (w *Worker) Name(`
            let after = if lang == MicroLang::Go && after.starts_with('(') {
                match after.find(')') {
                    Some(close) => after[close + 1..].trim_start(),
                    None => continue,
                }
            } else {
                after
            };
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn synth_fn_block(src: &mut Source, lang: MicroLang, callee: Option<&str>) -> String {
    let name = src.ident("mut_fn");
    let delta = (src.usize_below(31) + 1) as i32;
    let call_expr = match callee {
        Some(c) => format!("{c}(x) + {delta}"),
        None => format!("x + {delta}"),
    };
    match lang {
        MicroLang::Rust => {
            if src.bool() {
                format!("pub fn {name}(x: i32) -> i32 {{\n    {call_expr}\n}}")
            } else {
                format!("fn {name}(x: i32) -> i32 {{\n    {call_expr}\n}}")
            }
        }
        MicroLang::TypeScript => {
            if src.bool() {
                format!("export function {name}(x: number): number {{\n  return {call_expr};\n}}")
            } else {
                format!("function {name}(x: number): number {{\n  return {call_expr};\n}}")
            }
        }
        MicroLang::Go => {
            let go_name = if src.bool() {
                format!("MutFn_{}", src.usize_below(10_000))
            } else {
                name
            };
            format!("func {go_name}(x int) int {{\n\treturn {call_expr}\n}}")
        }
        MicroLang::Python => {
            format!("def {name}(x: int) -> int:\n    return {call_expr}")
        }
    }
}

/// Structural element-level mutation on a source file:
/// adds, deletes, duplicates (with renamed symbol), swaps top-level blocks, or
/// mutates an inline call/literal while keeping the file syntactically valid.
pub fn mutate_file_elements(src: &mut Source, lang: MicroLang, content: &str) -> String {
    let mut blocks: Vec<String> = content
        .split("\n\n")
        .map(|b| b.trim_matches('\n').to_string())
        .filter(|b| !b.is_empty())
        .collect();

    if blocks.is_empty() {
        return synth_fn_block(src, lang, None);
    }

    let first_body = usize::from(is_header_block(lang, &blocks[0]));
    let existing_fns: Vec<String> = blocks
        .iter()
        .filter_map(|b| extract_first_fn_name(lang, b))
        .collect();

    match src.usize_below(5) {
        // 0: Add a new function block (optionally calling an existing symbol).
        0 => {
            let callee = src.choose(&existing_fns).map(String::as_str);
            let new_block = synth_fn_block(src, lang, callee);
            let insert_at = if first_body < blocks.len() {
                first_body + src.usize_below(blocks.len() - first_body + 1)
            } else {
                blocks.len()
            };
            blocks.insert(insert_at, new_block);
        }
        // 1: Delete one non-header block (keep at least one body block).
        1 => {
            if blocks.len() > first_body + 1 {
                let drop_idx = first_body + src.usize_below(blocks.len() - first_body);
                blocks.remove(drop_idx);
            } else {
                let callee = src.choose(&existing_fns).map(String::as_str);
                blocks.push(synth_fn_block(src, lang, callee));
            }
        }
        // 2: Duplicate a function block with a fresh renamed identifier.
        2 => {
            if blocks.len() > first_body {
                let idx = first_body + src.usize_below(blocks.len() - first_body);
                if let Some(old_name) = extract_first_fn_name(lang, &blocks[idx]) {
                    let suffix = src.usize_below(10_000);
                    let new_name = format!("{old_name}_dup_{suffix}");
                    let dup = blocks[idx].replacen(&old_name, &new_name, 1);
                    blocks.insert(idx + 1, dup);
                } else {
                    let callee = src.choose(&existing_fns).map(String::as_str);
                    blocks.push(synth_fn_block(src, lang, callee));
                }
            }
        }
        // 3: Swap two non-header blocks.
        3 => {
            let body_count = blocks.len().saturating_sub(first_body);
            if body_count >= 2 {
                let a = first_body + src.usize_below(body_count);
                let mut b = first_body + src.usize_below(body_count);
                if a == b {
                    b = first_body + ((a - first_body + 1) % body_count);
                }
                blocks.swap(a, b);
            } else {
                let callee = src.choose(&existing_fns).map(String::as_str);
                blocks.push(synth_fn_block(src, lang, callee));
            }
        }
        // 4: Tweak a body line by adding a local binding or call to an existing symbol.
        _ => {
            let body_count = blocks.len().saturating_sub(first_body);
            if body_count > 0 {
                let idx = first_body + src.usize_below(body_count);
                let val = (src.usize_below(50) + 1) as i32;
                let callee = src.choose(&existing_fns).cloned();
                let mut lines: Vec<String> = blocks[idx].lines().map(String::from).collect();
                if lines.len() >= 2 {
                    let inject = match lang {
                        MicroLang::Rust => match callee {
                            Some(c) => format!("    let _t = {c}({val});"),
                            None => format!("    let _t = {val};"),
                        },
                        MicroLang::TypeScript => match callee {
                            Some(c) => format!("  const _t = {c}({val});"),
                            None => format!("  const _t = {val};"),
                        },
                        MicroLang::Go => match callee {
                            Some(c) => format!("\t_ = {c}({val})"),
                            None => format!("\t_ = {val}"),
                        },
                        MicroLang::Python => match callee {
                            Some(c) => format!("    _t = {c}({val})"),
                            None => format!("    _t = {val}"),
                        },
                    };
                    // Insert right after the opening signature line if it's a fn block.
                    if extract_first_fn_name(lang, &blocks[idx]).is_some() {
                        lines.insert(1, inject);
                        blocks[idx] = lines.join("\n");
                    }
                }
            }
        }
    }

    let mut joined = blocks.join("\n\n");
    joined.push('\n');
    joined
}

/// Grammar-lowering damage operator: deliberately breaks grammar
/// structure or injects adversarial bytes/Unicode while keeping valid UTF-8.
pub fn lower_damage(src: &mut Source, content: &str) -> String {
    if content.is_empty() {
        return "{{{ \"unclosed\0\r\n".to_string();
    }

    match src.usize_below(6) {
        // 0: Drop one structural delimiter (`{`, `}`, `(`, `)`, `;`, `:`).
        0 => {
            let delims: Vec<usize> = content
                .char_indices()
                .filter_map(|(i, c)| matches!(c, '{' | '}' | '(' | ')' | ';' | ':').then_some(i))
                .collect();
            if let Some(&pos) = src.choose(&delims) {
                let mut out = String::with_capacity(content.len());
                out.push_str(&content[..pos]);
                out.push_str(&content[pos + 1..]);
                out
            } else {
                format!("{content}\n{{")
            }
        }
        // 1: Truncate at a random UTF-8 char boundary.
        1 => {
            let cut = prev_char_boundary(content, src.usize_below(content.len()));
            content[..cut].to_string()
        }
        // 2: Insert unclosed string / brace / block comment at a char boundary.
        2 => {
            let tokens = [
                "\"unclosed_string",
                "/* unclosed comment",
                "{{{",
                "(((",
                "'''unclosed",
                "fn broken(",
            ];
            let tok = src.choose(&tokens).copied().unwrap_or("{{{");
            let cut = prev_char_boundary(content, src.usize_below(content.len() + 1));
            let mut out = String::with_capacity(content.len() + tok.len() + 2);
            out.push_str(&content[..cut]);
            out.push('\n');
            out.push_str(tok);
            out.push('\n');
            out.push_str(&content[cut..]);
            out
        }
        // 3: Prepend UTF-8 BOM, convert LF -> CRLF, and inject RTL override.
        3 => {
            let crlf = content.replace('\n', "\r\n");
            format!("\u{FEFF}// \u{202E}rtl\u{200B}\r\n{crlf}")
        }
        // 4: Inject embedded NUL and control characters at a char boundary.
        4 => {
            let cut = prev_char_boundary(content, src.usize_below(content.len() + 1));
            let mut out = String::with_capacity(content.len() + 8);
            out.push_str(&content[..cut]);
            out.push_str("\0\x07\x1b");
            out.push_str(&content[cut..]);
            out
        }
        // 5: Splice deeply nested unclosed brackets (bounded depth <= 48).
        _ => {
            let depth = 8 + src.usize_below(40);
            let open: String = std::iter::repeat_n('(', depth).collect();
            let cut = prev_char_boundary(content, src.usize_below(content.len() + 1));
            let mut out = String::with_capacity(content.len() + depth + 2);
            out.push_str(&content[..cut]);
            out.push_str(&open);
            out.push_str(&content[cut..]);
            out
        }
    }
}
