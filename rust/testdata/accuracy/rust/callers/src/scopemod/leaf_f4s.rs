use super::*;

#[test]
fn leaf_f4s() {
    let _s = Scoped::new_f4s();
}

#[test]
fn leaf_named_f4s() {
    let _q = Quarantine::new_f4();
}
