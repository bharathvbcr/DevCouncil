use std::fmt::Display;
pub struct Service<T>(T);
impl<T: Display> Service<T> { pub fn run(&self) { println!("{}", self.0); } }
