use crate::helper::helper;

pub trait Render {
    fn render(&self) -> String;
}

pub struct Widget {
    name: String,
}

impl Render for Widget {
    fn render(&self) -> String {
        helper(&self.name)
    }
}

pub fn main_entry() {
    let w = Widget { name: String::new() };
    w.render();
}
