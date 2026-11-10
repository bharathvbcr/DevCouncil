//! F2 near-miss: two traits are both called `Twin`. The call here names this
//! file's `Twin`; the impls of the other file's `Twin` are not its targets.
pub trait Twin {
    fn go_f2t(&self) -> u8;
}

pub struct SideA;

impl Twin for SideA {
    fn go_f2t(&self) -> u8 {
        1
    }
}

pub fn call_a_f2t(twin: &dyn Twin) -> u8 {
    twin.go_f2t()
}
