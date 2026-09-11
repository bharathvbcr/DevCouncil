fn health() {}
fn main() {
    let _ = Router::new().route("/health", get(health));
}
