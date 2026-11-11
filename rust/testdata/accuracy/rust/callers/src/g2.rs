//! G2: a file-level `Dup`, and a test module that imports a different one.
pub struct Dup;

impl Dup {
    pub fn new_g2() -> Dup {
        Dup
    }
}

pub fn outer_g2() -> Dup {
    Dup::new_g2()
}

#[cfg(test)]
mod tests {
    use crate::g2db::Dup;

    #[test]
    fn t_g2() {
        let _ = Dup::new_g2();
    }
}
