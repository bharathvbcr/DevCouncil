use rootpkg::hooks::{self};

fn main() {
    let a = hooks::dispatch("a");
    let b = rootpkg::hooks::dispatch("b");
    println!("{}", a + b);
}
