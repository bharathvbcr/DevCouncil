pub fn h52a() -> u16 { 2 }

pub fn mod_silent() -> u16 {
    println!("x");
    let _ = helper!();
    h52a()
}
