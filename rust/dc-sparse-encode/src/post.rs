//! The script's post-processing, after the forward and before the JSONL.
//!
//! `round(weight, 4)`, drop specials other than `[UNK]`, drop non-positive
//! weights, keep the heaviest 20,000, then sort by id. The 20,000 ceiling is
//! the one `dcgrep` refuses past, so applying it here is what makes a long
//! document an encoding rather than a refused build.

use std::collections::HashSet;

/// Terms `dcgrep` will accept from one document.
pub const MAX_TERMS_PER_DOCUMENT: usize = 20_000;

/// Python `round(x, 4)`: the exact value of the f64, to a multiple of
/// `10^-4`, half to even, then parsed back as a decimal with four places.
///
/// Scaling in f64 first is a different function. `0.00025 * 10000` is the
/// tie `2.5`, and half-away and half-even disagree on it, while the f64
/// itself is not the decimal a person typed. CPython rounds the exact
/// rational value (`Objects/floatobject.c`, `double_round` with dtoa mode 3).
pub fn round4(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let neg = x.is_sign_negative();
    let ax = x.abs();
    if ax == 0.0 {
        return 0.0;
    }
    let bits = ax.to_bits();
    let mut exp = ((bits >> 52) & 0x7ff) as i32;
    let mut mant = bits & ((1u64 << 52) - 1);
    if exp == 0 {
        // Subnormal: no hidden 1. The least subnormal is 2^(1-1023-52).
        exp = 1 - 1023 - 52;
    } else {
        mant |= 1u64 << 52;
        exp -= 1023 + 52;
    }
    // ax = mant * 2^exp, so ax * 10^4 = mant * 625 * 2^(exp+4).
    let num = (mant as u128).saturating_mul(625);
    let shift = exp + 4;
    let k = if shift >= 0 {
        let s = shift as u32;
        if s >= 128 || num.leading_zeros() < s {
            // Far outside a weight this encoder emits. Leave it unrounded
            // rather than wrapping the shift.
            return x;
        }
        num << s
    } else {
        let r = (-shift) as u32;
        if r == 0 || r >= 128 {
            if r == 0 { num } else { 0 }
        } else {
            let whole = num >> r;
            let frac = num & ((1u128 << r) - 1);
            let half = 1u128 << (r - 1);
            if frac > half || (frac == half && (whole & 1) == 1) {
                whole + 1
            } else {
                whole
            }
        }
    };
    let int_part = k / 10_000;
    let frac = (k % 10_000) as u32;
    let sign = if neg { "-" } else { "" };
    format!("{sign}{int_part}.{frac:04}")
        .parse::<f64>()
        .unwrap_or(x)
}

/// One row of pooled weights to `(id, rounded weight)`, id order.
///
/// `row[i] == 0` is skipped, which is `torch.nonzero`. A weight that rounds
/// to zero or below is skipped after that, which is the script's `w > 0`
/// filter. Specials are dropped by id, never by assuming the id numbers:
/// `[UNK]` stays, because it is a fact about the document.
pub fn terms_of(row: &[f32], drop: &HashSet<u32>) -> Vec<(u32, f64)> {
    let mut pairs = Vec::new();
    for (i, &weight) in row.iter().enumerate() {
        if weight == 0.0 || !weight.is_finite() {
            continue;
        }
        let id = i as u32;
        if drop.contains(&id) {
            continue;
        }
        let rounded = round4(f64::from(weight));
        if rounded > 0.0 {
            pairs.push((id, rounded));
        }
    }
    if pairs.len() > MAX_TERMS_PER_DOCUMENT {
        // Stable: equal weights keep the id order `nonzero` produced, and
        // the cutoff therefore prefers the lower id. Sorting by id as a
        // tiebreak would do the same; the stable sort is what the script does.
        pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        pairs.truncate(MAX_TERMS_PER_DOCUMENT);
    }
    pairs.sort_by_key(|pair| pair.0);
    pairs
}

/// Ids of `[PAD]`, `[CLS]`, `[SEP]` and `[MASK]`. `[UNK]` is not in the set.
pub fn special_drop(vocab: &[String]) -> Result<HashSet<u32>, String> {
    let mut drop = HashSet::new();
    let mut unk = false;
    for (i, token) in vocab.iter().enumerate() {
        match token.as_str() {
            "[PAD]" | "[CLS]" | "[SEP]" | "[MASK]" => {
                drop.insert(i as u32);
            }
            "[UNK]" => unk = true,
            _ => {}
        }
    }
    if drop.len() != 4 || !unk {
        return Err(
            "vocabulary is missing one of [PAD], [UNK], [CLS], [SEP], [MASK]; \
             the specials this encoder drops are those four, and [UNK] is kept"
                .into(),
        );
    }
    Ok(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These are `python3 -c "round(x, 4)"` on 3.14, including the ties where
    /// half-even and half-away disagree. A `round()` of the scaled f64 gets
    /// `0.00035` wrong (it goes to 0.0004).
    #[test]
    fn round4_matches_cpython() {
        let cases = [
            (0.0, 0.0),
            (4e-5, 0.0),
            (5e-5, 0.0001),
            (6e-5, 0.0001),
            (0.00015, 0.0001),
            (0.00025, 0.0003),
            (0.00035, 0.0003),
            (0.00045, 0.0004),
            (0.00055, 0.0006),
            (0.00065, 0.0006),
            (0.00075, 0.0008),
            (0.00085, 0.0008),
            (0.00095, 0.0009),
            (1.225, 1.225),
            (2.675, 2.675),
            (0.9404440522193909, 0.9404),
            (-0.00025, -0.0003),
            (-0.00035, -0.0003),
        ];
        for (input, want) in cases {
            let got = round4(input);
            assert!(
                (got - want).abs() < 1e-12,
                "round4({input}) = {got}, python round is {want}"
            );
        }
    }

    #[test]
    fn terms_drop_specials_keep_unk_and_cap_at_the_heaviest() {
        let vocab = vec![
            "[PAD]".into(),
            "[UNK]".into(),
            "a".into(),
            "[CLS]".into(),
            "[SEP]".into(),
            "[MASK]".into(),
        ];
        let drop = special_drop(&vocab).unwrap();
        // Index 0, 3, 4, 5 are specials with large weights. 1 is [UNK].
        let mut row = vec![0.0f32; 6];
        row[0] = 9.0;
        row[1] = 0.5;
        row[2] = 0.00004; // rounds to 0
        row[3] = 8.0;
        row[4] = 7.0;
        row[5] = 6.0;
        assert_eq!(terms_of(&row, &drop), vec![(1, 0.5)]);

        let mut wide = vec![0.0f32; 20_010];
        for (i, weight) in wide.iter_mut().enumerate() {
            *weight = (i as f32) * 0.001;
        }
        // The specials sit at the front with tiny weights, so the cap keeps
        // the tail. Give the specials nothing and the low ids a real weight
        // that must lose to the tail.
        let drop = HashSet::new();
        let kept = terms_of(&wide, &drop);
        assert_eq!(kept.len(), MAX_TERMS_PER_DOCUMENT);
        assert!(kept.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(kept[0].0, 10); // 0..9 rounded away or lost the cutoff
        assert_eq!(*kept.last().unwrap(), (20_009, round4(f64::from(20_009.0 * 0.001))));
    }
}
