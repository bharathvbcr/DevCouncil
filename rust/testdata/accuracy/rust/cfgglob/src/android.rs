macro_rules! mk {
    () => {};
}
mk! {}

pub struct Inner;

impl Inner {
    pub fn android_only() -> u8 {
        1
    }
}
