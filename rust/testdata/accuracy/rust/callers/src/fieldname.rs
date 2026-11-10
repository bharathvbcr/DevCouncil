//! F8: `self.attended.lock()` reads a FIELD named like the method `attended`.
use std::sync::Mutex;

pub struct Recorder {
    attended: Mutex<bool>,
}

impl Recorder {
    pub fn attended(&self) -> bool {
        *self.attended.lock().unwrap()
    }

    pub fn flip_f8(&self) {
        let mut v = self.attended.lock().unwrap();
        *v = !*v;
    }

    pub fn check_f8(&self) -> bool {
        self.attended()
    }
}

/// A second type with a method of the same name: the field read in `Recorder`
/// must not bind here either.
pub struct Observer;

impl Observer {
    pub fn attended(&self) -> bool {
        false
    }
}

pub fn use_f8(r: &Recorder, o: &Observer) -> bool {
    r.flip_f8();
    r.check_f8() && o.attended()
}

pub struct Holder {
    inner: Recorder,
}

/// A field read through a parameter and through a chain: neither is a call of
/// the method `attended`.
pub fn peek_f8(r: &Recorder, h: &Holder) -> bool {
    let a = *r.attended.lock().unwrap();
    let b = *h.inner.attended.lock().unwrap();
    a && b
}
