//! C21 integration-test entry points in tests/.

#[test]
fn it_c21() {
    assert_eq!(corelib::target_c21(), 21);
}

#[tokio::test]
async fn it2_c21() {
    assert_eq!(corelib::asyncs::fetch_c16().await, 16);
}
