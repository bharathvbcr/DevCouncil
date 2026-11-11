//! `thread_local!` declares exactly the statics it names, and `bail!`/`ensure!`
//! declare nothing: neither leaves the module's other names unplaced. Each call
//! sits in the same block as the macro, so only the macro rule can answer it.
use std::cell::Cell;

pub fn h44() -> u16 {
    2
}

pub fn after_thread_local() -> u16 {
    thread_local! {
        static TL44: Cell<u8> = const { Cell::new(1) };
    }
    h44()
}

pub fn h44b() -> u16 {
    3
}

pub fn after_bail(flag: bool) -> Result<u16, String> {
    let kept = h44b();
    anyhow::ensure!(!flag, "no");
    Ok(kept)
}

pub fn h44c() -> u16 {
    4
}

pub fn after_bail_tail(flag: bool) -> Result<u16, String> {
    let kept = h44c();
    if flag {
        return Ok(kept);
    }
    anyhow::bail!("no")
}
