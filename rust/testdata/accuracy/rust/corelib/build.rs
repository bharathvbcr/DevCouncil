// C22 build.rs entry point
fn main() {
    gen_c22();
}

fn gen_c22() {
    println!("cargo:rerun-if-changed=build.rs");
}
