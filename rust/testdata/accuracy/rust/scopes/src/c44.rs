//! `thread_local!` declares exactly the statics it names, and `bail!` declares
//! nothing: neither leaves the module's other names unplaced.
use std::cell::Cell;

thread_local! {
    static TL44: Cell<u8> = const { Cell::new(1) };
}

pub fn h44() -> u16 { 2 }

pub fn after_thread_local() -> u16 {
    h44()
}

pub fn h44b() -> u16 { 3 }

pub fn after_bail(flag: bool) -> Result<u16, String> {
    if flag {
        anyhow::bail!("no");
    }
    Ok(h44b())
}
