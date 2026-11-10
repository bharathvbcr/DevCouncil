use super::*;

fn helper_f4b() -> u32 {
    2
}

#[test]
fn child_f4b() {
    assert_eq!(helper_f4b(), 2);
}
