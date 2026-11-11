//! G4: `helper_g4` comes from a glob at file level; the only declaration of the
//! name in this file is inside `mod tests`.
use crate::util_g4::*;

pub fn top_g4() -> u32 {
    helper_g4()
}

#[cfg(test)]
mod tests {
    fn helper_g4() -> u32 {
        2
    }

    #[test]
    fn t_g4() {
        assert_eq!(helper_g4(), 2);
    }
}
