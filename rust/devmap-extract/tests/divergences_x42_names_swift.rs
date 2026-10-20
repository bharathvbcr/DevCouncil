//! `docs/devmap/DIVERGENCES.md` is where a deliberate behaviour is written down,
//! and X42 (an implicit receiver dispatches on the enclosing type) is the row
//! Swift's `deinit` bodies and extension members are answered under. The row
//! named only `self`/`cls`/`this` and Go receivers, so a reader looking for why
//! `deinit { cleanup() }` reaches `Session.cleanup` — declared in an extension,
//! maybe in another file — found nothing. The fixture rows sw.X42a-e pin the
//! behaviour; this pins the sentence that explains it.

const DIVERGENCES: &str = include_str!("../../../docs/devmap/DIVERGENCES.md");

fn x42_row() -> &'static str {
    DIVERGENCES
        .lines()
        .find(|line| line.starts_with("| **X42 "))
        .expect("DIVERGENCES.md carries an X42 row")
}

#[test]
fn x42_names_swift_deinit_bodies() {
    let row = x42_row();
    assert!(
        row.contains("`deinit`"),
        "the X42 row must say a Swift `deinit` body's implicit-self call dispatches on the owning type"
    );
}

#[test]
fn x42_names_extension_members_in_other_files() {
    let row = x42_row();
    assert!(
        row.contains("extension") && row.contains("another file"),
        "the X42 row must say implicit self reaches members declared in an extension in another file"
    );
}
