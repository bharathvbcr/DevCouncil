//! Gusset umbrella for the DevCouncil fnmatch engine.
//!
//! cgo links `-lgusset`, so this crate's archive is `libgusset.a` (`[lib] name`,
//! not the package name). It is the only Rust staticlib a Go binary in this
//! suite may link. A second archive defines `rust_eh_personality` twice.

use gusset_core::{register_engine, set_engine_handler, Counting, JobContext};

/// Counts every Rust allocation in the host process.
///
/// Gusset declares no global allocator, so without this `gusset.Stats()`
/// reads zero and `AdviseMemoryLimit` subtracts nothing: the Go side of every
/// binary that links this archive was budgeting as if Rust used no memory.
/// The umbrella is the one archive per binary (R14), so this is the one place
/// it can be declared.
#[global_allocator]
static ALLOCATOR: Counting<std::alloc::System> = Counting::new(std::alloc::System);

/// Largest pattern or name accepted across the boundary, in bytes.
///
/// dc-glob's own cap is 16384 Unicode scalar values, at most four bytes each.
/// Go's fnmatch matches exactly that much, and the policy gate now routes
/// through this engine, so a field Go would match must reach it: the cap used
/// to be 1024 bytes, which would have refused an ordinary long command. Past
/// dc-glob's own cap the answer is [`UNDECIDED`], never an error.
pub const MAX_FIELD: usize = 16_384 * 4;

/// Answer bytes. [`UNDECIDED`] is dc-glob's `None`: the input was past its cap
/// or the walk exhausted its budget. Go reads it as no match for `Match` and
/// as a match for `MatchFold`, the deny-list entry point.
pub const NO_MATCH: u8 = 0;
/// See [`NO_MATCH`].
pub const MATCH: u8 = 1;
/// See [`NO_MATCH`].
pub const UNDECIDED: u8 = 2;

/// Opcode for [`match_any_frame`]: one crossing for a whole pattern list.
///
/// Opcode 0 is the global handler ([`match_frame`]). Gusset resolves a
/// registered opcode before the global handler, so this cannot shadow it.
pub const OPCODE_MATCH_ANY: u32 = 1;

/// Largest pattern count in one match-any frame; Go splits longer lists.
pub const MAX_PATTERNS: usize = 1024;

/// Opcode that panics on purpose, so the host can prove the panic firewall
/// (I2) against this archive rather than against Gusset's diagnostic engine.
///
/// Reachable only through the call header, which Go sets; no payload byte
/// selects it. `gussetfn.SelfTest` sends it on a throwaway handle and requires
/// ErrPanic, then ErrPoisoned, then a clean match on the shared handle;
/// `gussetfn.Match` pins opcode 0 so a context carrying this one cannot
/// reach the shared handle. `Check` never sends it.
pub const OPCODE_SELF_TEST_PANIC: u32 = 0x7fff_0001;

/// Registers the dc-glob engine on Gusset's global handler.
///
/// Call once, before the first submission. Registration is not a Gusset export;
/// the umbrella owns it. The diagnostic engine stays unreachable: this handler
/// is installed unconditionally and a stray diagnostic flag cannot displace it.
///
/// # Safety
///
/// The Go process that linked this archive calls this once, before `gusset_submit`.
#[no_mangle]
pub unsafe extern "C" fn devcouncil_gusset_init() -> i32 {
    set_engine_handler(
        |ctx: &JobContext, input: &[u8]| -> Result<Vec<u8>, String> {
            ctx.check()
                .map_err(|reason| format!("cancelled: {:?}", reason))?;
            match_frame(input)
        },
    );
    register_engine(
        OPCODE_MATCH_ANY,
        |ctx: &JobContext, input: &[u8]| -> Result<Vec<u8>, String> {
            ctx.check()
                .map_err(|reason| format!("cancelled: {:?}", reason))?;
            match_any_frame(input, || {
                ctx.check()
                    .map_err(|reason| format!("cancelled: {:?}", reason))
            })
        },
    );
    register_engine(
        OPCODE_SELF_TEST_PANIC,
        |_: &JobContext, _: &[u8]| -> Result<Vec<u8>, String> {
            panic!("devcouncil gusset self-test panic");
        },
    );
    0
}

/// Reads a `u32le` at `at`.
fn read_u32(input: &[u8], at: usize, what: &str) -> Result<(usize, usize), String> {
    let end = at
        .checked_add(4)
        .filter(|&end| end <= input.len())
        .ok_or_else(|| format!("truncated {what}"))?;
    let v = u32::from_le_bytes([input[at], input[at + 1], input[at + 2], input[at + 3]]);
    Ok((v as usize, end))
}

/// Reads one `u32le len | bytes` field at `at`, returning it and the next offset.
fn field<'a>(input: &'a [u8], at: usize, what: &str) -> Result<(&'a str, usize), String> {
    let (len, len_end) = read_u32(input, at, &format!("{what} length"))?;
    if len > MAX_FIELD {
        return Err(format!("{what} length {len} exceeds {MAX_FIELD} bytes"));
    }
    let end = len_end
        .checked_add(len)
        .filter(|&end| end <= input.len())
        .ok_or_else(|| format!("truncated {what}"))?;
    let text = std::str::from_utf8(&input[len_end..end])
        .map_err(|err| format!("{what} is not valid UTF-8: {err}"))?;
    Ok((text, end))
}

fn answer(decided: Option<bool>) -> u8 {
    match decided {
        Some(true) => MATCH,
        Some(false) => NO_MATCH,
        None => UNDECIDED,
    }
}

/// Decodes a match-any frame and returns one answer byte.
///
/// Frame: `u32le name_len | name | u32le count | (u32le pattern_len | pattern){count}`,
/// nothing else. [`MATCH`] when any pattern matches; otherwise [`UNDECIDED`]
/// when any could not be decided; otherwise [`NO_MATCH`]. Every field is
/// validated before any is matched, so a bad field late in the list is an
/// error rather than a result that depends on list order. `check` runs
/// between patterns, so a long list observes cancellation.
pub fn match_any_frame(
    input: &[u8],
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    let (name, at) = field(input, 0, "name")?;
    let (count, mut at) = read_u32(input, at, "pattern count")?;
    if count > MAX_PATTERNS {
        return Err(format!("pattern count {count} exceeds {MAX_PATTERNS}"));
    }
    let mut patterns = Vec::with_capacity(count);
    for _ in 0..count {
        let (pattern, next) = field(input, at, "pattern")?;
        patterns.push(pattern);
        at = next;
    }
    if at != input.len() {
        return Err("match-any frame length does not equal its headers".to_string());
    }
    let mut undecided = false;
    for pattern in patterns {
        check()?;
        match dc_glob::try_matches(pattern, name) {
            Some(true) => return Ok(vec![MATCH]),
            Some(false) => {}
            None => undecided = true,
        }
    }
    Ok(vec![if undecided { UNDECIDED } else { NO_MATCH }])
}

/// Decodes a match frame and returns one answer byte.
///
/// Frame: `u32le pattern_len | pattern | u32le name_len | name`, nothing else.
/// Invalid UTF-8, a short buffer, a trailing byte, or a field over
/// [`MAX_FIELD`] is an engine error. None of those panic, so the handle stays
/// usable.
pub fn match_frame(input: &[u8]) -> Result<Vec<u8>, String> {
    let (pattern, at) = field(input, 0, "pattern")?;
    let (name, at) = field(input, at, "name")?;
    if at != input.len() {
        return Err("match frame length does not equal its headers".to_string());
    }
    Ok(vec![answer(dc_glob::try_matches(pattern, name))])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(out: &mut Vec<u8>, field: &[u8]) {
        out.extend_from_slice(&(field.len() as u32).to_le_bytes());
        out.extend_from_slice(field);
    }

    fn frame(pattern: &[u8], name: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, pattern);
        put(&mut out, name);
        out
    }

    fn any_frame(name: &[u8], patterns: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, name);
        out.extend_from_slice(&(patterns.len() as u32).to_le_bytes());
        for p in patterns {
            put(&mut out, p);
        }
        out
    }

    fn ok() -> Result<(), String> {
        Ok(())
    }

    #[test]
    fn match_any_matches_one_of_the_list() {
        let got = match_any_frame(&any_frame(b"src/foo.py", &[b"*.rs", b"*.py"]), ok).unwrap();
        assert_eq!(got, vec![MATCH]);
        let got = match_any_frame(&any_frame(b"src/foo.js", &[b"*.rs", b"*.py"]), ok).unwrap();
        assert_eq!(got, vec![NO_MATCH]);
    }

    #[test]
    fn match_any_of_nothing_is_no_match() {
        assert_eq!(
            match_any_frame(&any_frame(b"a", &[]), ok).unwrap(),
            vec![NO_MATCH]
        );
    }

    // A match beats an undecided pattern; an undecided pattern beats no match.
    // Go reads UNDECIDED per entry point, so collapsing it to NO_MATCH would
    // turn a deny-list's fail-closed answer into an allow.
    #[test]
    fn match_any_reports_undecided_unless_something_matched() {
        let long = "a".repeat(16_385);
        let got = match_any_frame(&any_frame(b"a.py", &[long.as_bytes(), b"*.rs"]), ok).unwrap();
        assert_eq!(got, vec![UNDECIDED]);
        let got = match_any_frame(&any_frame(b"a.py", &[long.as_bytes(), b"*.py"]), ok).unwrap();
        assert_eq!(got, vec![MATCH]);
    }

    #[test]
    fn match_frame_reports_undecided_past_the_cap() {
        let long = "a".repeat(16_385);
        assert_eq!(
            match_frame(&frame(long.as_bytes(), b"a")).unwrap(),
            vec![UNDECIDED]
        );
        assert_eq!(
            match_frame(&frame(b"a", long.as_bytes())).unwrap(),
            vec![UNDECIDED]
        );
    }

    // The cap used to be 1024 bytes, which refused a long command Go matched.
    #[test]
    fn a_field_up_to_the_cap_is_matched() {
        let name = format!("git commit -m {}", "x".repeat(8_000));
        assert_eq!(
            match_frame(&frame(b"git commit *", name.as_bytes())).unwrap(),
            vec![MATCH]
        );
    }

    #[test]
    fn match_any_validates_every_pattern_before_matching() {
        // The first pattern matches; the second is bad UTF-8. A short-circuit
        // would answer MATCH and hide the malformed field.
        let err = match_any_frame(&any_frame(b"a.py", &[b"*.py", b"\xff"]), ok).unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[test]
    fn match_any_observes_cancellation_between_patterns() {
        let mut calls = 0;
        let err = match_any_frame(&any_frame(b"a", &[b"b", b"c", b"a"]), || {
            calls += 1;
            if calls == 2 {
                Err("cancelled: Explicit".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(err.contains("cancelled"), "{err}");
    }

    #[test]
    fn match_any_refuses_truncation_and_trailing_bytes() {
        let framed = any_frame(b"a", &[b"a", b"b"]);
        for cut in 0..framed.len() {
            assert!(
                match_any_frame(&framed[..cut], ok).is_err(),
                "prefix of {cut} bytes"
            );
        }
        let mut long = framed.clone();
        long.push(0);
        assert!(match_any_frame(&long, ok)
            .unwrap_err()
            .contains("does not equal"));
    }

    #[test]
    fn match_any_refuses_a_count_past_the_cap() {
        let mut framed = Vec::new();
        put(&mut framed, b"a");
        framed.extend_from_slice(&((MAX_PATTERNS + 1) as u32).to_le_bytes());
        assert!(match_any_frame(&framed, ok)
            .unwrap_err()
            .contains("exceeds"));
    }

    #[test]
    fn no_prefix_of_a_match_frame_panics() {
        let framed = frame(b"src/*.py", b"src/foo.py");
        for cut in 0..framed.len() {
            assert!(
                match_frame(&framed[..cut]).is_err(),
                "prefix of {cut} bytes"
            );
        }
    }

    #[test]
    fn star_crosses_a_separator() {
        assert_eq!(
            match_frame(&frame(b"*.py", b"src/foo.py")).unwrap(),
            vec![MATCH]
        );
    }

    #[test]
    fn star_does_not_match_a_different_suffix() {
        assert_eq!(
            match_frame(&frame(b"*.py", b"src/foo.rs")).unwrap(),
            vec![NO_MATCH]
        );
    }

    #[test]
    fn invalid_utf8_is_an_error_not_a_panic() {
        let err = match_frame(&frame(b"\xff", b"a")).unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut framed = frame(b"a", b"a");
        framed.push(0);
        assert!(match_frame(&framed).unwrap_err().contains("does not equal"));
    }

    #[test]
    fn a_field_past_the_byte_cap_is_refused_without_parsing_it() {
        let mut framed = Vec::new();
        framed.extend_from_slice(&((MAX_FIELD + 1) as u32).to_le_bytes());
        framed.extend(std::iter::repeat_n(b'a', 4));
        let err = match_frame(&framed).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }
}
