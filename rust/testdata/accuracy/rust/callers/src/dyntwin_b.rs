pub trait Twin {
    fn go_f2t(&self) -> u8;
}

pub struct SideB;

impl Twin for SideB {
    fn go_f2t(&self) -> u8 {
        2
    }
}
