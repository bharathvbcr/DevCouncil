pub mod hooks {
    pub fn dispatch(name: &str) -> u8 {
        name.len() as u8
    }
}

pub fn count(items: &[u8]) -> usize {
    items.len()
}

pub fn word_chars(s: &str) -> usize {
    s.chars().count()
}
