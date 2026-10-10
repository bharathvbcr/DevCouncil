use xlib::engine::{only_from_bin_c44, Thing};

fn main() {
    let _ = xlib::run_c44(); // C44: bin -> lib through the [lib] name
    let t = Thing::make_c44();
    let _ = t.value_c44();
    let _ = only_from_bin_c44();
}
