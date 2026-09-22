//! Gusset umbrella for the DevCouncil fnmatch engine.
//!
//! cgo links `-lgusset`, so this crate's archive is `libgusset.a` (`[lib] name`,
//! not the package name). It is the only Rust staticlib a Go binary in this
//! suite may link. A second archive defines `rust_eh_personality` twice.

use gusset_core::{set_engine_handler, JobContext};

/// Largest pattern or name accepted across the boundary.
///
/// The whole frame stays under Gusset's 4 KiB inline-copy limit, so `Call`
/// never memcpy's a path on the cgo thread.
pub const MAX_FIELD: usize = 1024;

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
    0
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
        framed.extend(std::iter::repeat(b'a').take(4));
        let err = match_frame(&framed).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }
}
