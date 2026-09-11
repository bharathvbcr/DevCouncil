// Many corpus types declare `new`. An external std receiver must not fan out
// AmbiguousGlobal edges to every one of them.
pub struct Alpha;
impl Alpha {
    pub fn new() -> Self {
        Self
    }
}

pub struct Beta;
impl Beta {
    pub fn new() -> Self {
        Self
    }
}

pub struct Gamma;
impl Gamma {
    pub fn new() -> Self {
        Self
    }
}

pub fn make_string() -> String {
    String::new()
}

pub fn make_vec() -> Vec<u8> {
    Vec::new()
}
