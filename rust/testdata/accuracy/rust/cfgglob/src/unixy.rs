macro_rules! mk {
    () => {};
}
mk! {}

pub struct Inner;

impl Inner {
    pub fn unix_only() -> u8 {
        2
    }
}
