mod helpers;

fn main() {
    let _ = engine_core::run_cp();
    helpers::assist_cp();
    crate::helpers::finish_cp();
}
