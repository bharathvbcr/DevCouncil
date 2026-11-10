//! F2: a call through `Arc<dyn Host>` reaches the impls' methods.
use std::sync::Arc;

pub trait Host {
    fn attended_f2(&self, key: &str) -> bool;
}

pub struct RealHost;
pub struct StubHost;
pub struct Unrelated;

impl Host for RealHost {
    fn attended_f2(&self, _key: &str) -> bool {
        true
    }
}

impl Host for StubHost {
    fn attended_f2(&self, _key: &str) -> bool {
        false
    }
}

/// Has a method of the same name but does not implement `Host`.
impl Unrelated {
    pub fn attended_f2(&self, _key: &str) -> bool {
        false
    }
}

pub fn flush_f2(host: &Arc<dyn Host>) -> bool {
    host.attended_f2("k")
}

pub fn flush_ref_f2(host: &dyn Host) -> bool {
    host.attended_f2("k")
}
