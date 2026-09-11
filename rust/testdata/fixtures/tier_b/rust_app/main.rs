// Rust Audit Blind-spot Syntax Fixture (G19 generic impls, X9 grouped use)
use std::{fmt::Display, collections::{HashMap, HashSet}};

pub trait Processor<T> {
    fn process(&self, val: T) -> String;
}

pub struct GenericContainer<T> {
    pub item: T,
    pub map: HashMap<String, HashSet<usize>>,
}

impl<T: Display> Processor<T> for GenericContainer<T> {
    fn process(&self, val: T) -> String {
        format!("Item: {}, Val: {}", self.item, val)
    }
}

fn main() {
    let container = GenericContainer {
        item: 42,
        map: HashMap::new(),
    };
    println!("{}", container.process(100));
}
