//! An explicit import shadows a glob of the same name (rustc: the glob is reported
//! unused), so `bail!` stays anyhow's although `dep` exports one.
use anyhow::bail;
use dep::*;

pub fn h53() -> u16 {
    2
}

pub fn explicit_beats_glob(flag: bool) -> Result<u16, String> {
    let kept = h53();
    if flag {
        return Ok(kept);
    }
    bail!("no")
}
