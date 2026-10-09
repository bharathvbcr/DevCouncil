use xlib::engine::Thing;

#[test]
fn it_c44() {
    assert_eq!(Thing::make_c44().value_c44(), 44);
    assert_eq!(xlib::engine::only_from_bin_c44(), 1);
}
