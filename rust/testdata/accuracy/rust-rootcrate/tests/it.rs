use rootpkg::word_chars;

#[test]
fn counts_characters() {
    assert_eq!(word_chars("ab"), 2);
}
