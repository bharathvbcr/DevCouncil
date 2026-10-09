pub mod engine;

pub fn run_c44() -> u8 {
    engine::Thing::make_c44().value_c44()
}
