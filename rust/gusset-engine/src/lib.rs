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

/// Largest pattern or name accepted across the boundary.
///
/// The whole frame stays under Gusset's 4 KiB inline-copy limit, so `Call`
/// never memcpy's a path on the cgo thread.
pub const MAX_FIELD: usize = 1024;

/// Opcode for [`match_any_frame`]: one crossing for a whole pattern list.
///
/// Opcode 0 is the global handler ([`match_frame`]). Gusset resolves a
/// registered opcode before the global handler, so this cannot shadow it.
pub const OPCODE_MATCH_ANY: u32 = 1;

/// Largest pattern count in one match-any frame.
///
/// The 4 KiB inline limit already bounds a frame; this bounds the loop over
/// zero-length patterns, which cost two bytes each.
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
            match_any_frame(input)
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

/// Reads one `u16le len | bytes` field at `at`, returning it and the next offset.
fn field<'a>(input: &'a [u8], at: usize, what: &str) -> Result<(&'a str, usize), String> {
    let len_end = at
        .checked_add(2)
        .filter(|&end| end <= input.len())
        .ok_or_else(|| format!("truncated {what} length"))?;
    let len = u16::from_le_bytes([input[at], input[at + 1]]) as usize;
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

/// Decodes a match-any frame and returns a single byte, 1 when any pattern matches.
///
/// Frame: `u16le name_len | name | u16le count | (u16le pattern_len | pattern){count}`,
/// nothing else. Every pattern is validated before any is matched, so a bad
/// field late in the list is an error rather than a result that depends on
/// whether an earlier pattern happened to match first.
pub fn match_any_frame(input: &[u8]) -> Result<Vec<u8>, String> {
    let (name, mut at) = field(input, 0, "name")?;
    if at + 2 > input.len() {
        return Err("truncated pattern count".to_string());
    }
    let count = u16::from_le_bytes([input[at], input[at + 1]]) as usize;
    if count > MAX_PATTERNS {
        return Err(format!("pattern count {count} exceeds {MAX_PATTERNS}"));
    }
    at += 2;
    let mut patterns = Vec::with_capacity(count);
    for _ in 0..count {
        let (pattern, next) = field(input, at, "pattern")?;
        patterns.push(pattern);
        at = next;
    }
    if at != input.len() {
        return Err("match-any frame length does not equal its headers".to_string());
    }
    Ok(vec![u8::from(dc_glob::matches_any(&patterns, name))])
}

/// Decodes a length-prefixed frame and returns a single byte, 1 for match.
///
/// Frame: `u16le pattern_len | pattern | u16le name_len | name`, nothing else.
/// Invalid UTF-8, a short buffer, a trailing byte, or a field over [`MAX_FIELD`]
/// is an engine error. None of those panic, so the handle stays usable.
pub fn match_frame(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() < 4 {
        return Err("truncated match frame".to_string());
    }
    let pattern_len = u16::from_le_bytes([input[0], input[1]]) as usize;
    if pattern_len > MAX_FIELD {
        return Err(format!(
            "pattern length {pattern_len} exceeds {MAX_FIELD} bytes"
        ));
    }
    let name_len_at = 2 + pattern_len;
    if input.len() < name_len_at + 2 {
        return Err("truncated pattern in match frame".to_string());
    }
    let name_len = u16::from_le_bytes([input[name_len_at], input[name_len_at + 1]]) as usize;
    if name_len > MAX_FIELD {
        return Err(format!("name length {name_len} exceeds {MAX_FIELD} bytes"));
    }
    let name_at = name_len_at + 2;
    if input.len() != name_at + name_len {
        return Err("match frame length does not equal its headers".to_string());
    }
    let pattern = std::str::from_utf8(&input[2..name_len_at])
        .map_err(|err| format!("pattern is not valid UTF-8: {err}"))?;
    let name = std::str::from_utf8(&input[name_at..])
        .map_err(|err| format!("name is not valid UTF-8: {err}"))?;
    Ok(vec![u8::from(dc_glob::matches(pattern, name))])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pattern: &[u8], name: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(pattern.len() as u16).to_le_bytes());
        out.extend_from_slice(pattern);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
        out
    }

    fn any_frame(name: &[u8], patterns: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&(patterns.len() as u16).to_le_bytes());
        for p in patterns {
            out.extend_from_slice(&(p.len() as u16).to_le_bytes());
            out.extend_from_slice(p);
        }
        out
    }

    #[test]
    fn match_any_matches_one_of_the_list() {
        let got = match_any_frame(&any_frame(b"src/foo.py", &[b"*.rs", b"*.py"])).unwrap();
        assert_eq!(got, vec![1]);
        let got = match_any_frame(&any_frame(b"src/foo.js", &[b"*.rs", b"*.py"])).unwrap();
        assert_eq!(got, vec![0]);
    }

    #[test]
    fn match_any_of_nothing_is_no_match() {
        assert_eq!(match_any_frame(&any_frame(b"a", &[])).unwrap(), vec![0]);
    }

    #[test]
    fn match_any_validates_every_pattern_before_matching() {
        // The first pattern matches; the second is bad UTF-8. A short-circuit
        // would answer 1 and hide the malformed field.
        let err = match_any_frame(&any_frame(b"a.py", &[b"*.py", b"\xff"])).unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[test]
    fn match_any_refuses_truncation_and_trailing_bytes() {
        let framed = any_frame(b"a", &[b"a", b"b"]);
        for cut in 0..framed.len() {
            assert!(match_any_frame(&framed[..cut]).is_err(), "prefix of {cut} bytes");
        }
        let mut long = framed.clone();
        long.push(0);
        assert!(match_any_frame(&long).unwrap_err().contains("does not equal"));
    }

    #[test]
    fn match_any_refuses_a_count_past_the_cap() {
        let mut framed = vec![1, 0, b'a'];
        framed.extend_from_slice(&((MAX_PATTERNS + 1) as u16).to_le_bytes());
        assert!(match_any_frame(&framed).unwrap_err().contains("exceeds"));
    }

    #[test]
    fn no_prefix_of_a_match_frame_panics() {
        let framed = frame(b"src/*.py", b"src/foo.py");
        for cut in 0..framed.len() {
            assert!(match_frame(&framed[..cut]).is_err(), "prefix of {cut} bytes");
        }
    }

    #[test]
    fn star_crosses_a_separator() {
        let got = match_frame(&frame(b"*.py", b"src/foo.py")).unwrap();
        assert_eq!(got, vec![1]);
    }

    #[test]
    fn star_does_not_match_a_different_suffix() {
        let got = match_frame(&frame(b"*.py", b"src/foo.rs")).unwrap();
        assert_eq!(got, vec![0]);
    }

    #[test]
    fn invalid_utf8_is_an_error_not_a_panic() {
        let err = match_frame(&frame(b"\xff", b"a")).unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[test]
    fn a_short_frame_is_refused() {
        assert!(match_frame(&[0, 0, 0]).unwrap_err().contains("truncated"));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut framed = frame(b"a", b"a");
        framed.push(0);
        assert!(match_frame(&framed).unwrap_err().contains("does not equal"));
    }

    #[test]
    fn a_field_past_the_cap_is_refused_without_parsing_it() {
        let mut framed = vec![0x01, 0x04]; // 1025
        framed.extend(std::iter::repeat_n(b'a', 4));
        let err = match_frame(&framed).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }
}
