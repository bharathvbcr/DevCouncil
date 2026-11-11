//! Procedural macros that emit a `use` of the fixtures' second `Dup`.
use proc_macro::TokenStream;

/// Emits `use <args>;` beside the item it decorates.
#[proc_macro_attribute]
pub fn with_use(args: TokenStream, item: TokenStream) -> TokenStream {
    format!("use {}; {}", args, item).parse().unwrap()
}

/// Emits `use <args>;` as the first statement of the function it decorates.
#[proc_macro_attribute]
pub fn body_use(args: TokenStream, item: TokenStream) -> TokenStream {
    let text = item.to_string();
    let at = text.find('{').expect("a function with a body");
    format!("{} use {}; {}", &text[..=at], args, &text[at + 1..])
        .parse()
        .unwrap()
}

/// A derive that is not `Clone` although it is spelled that way: it emits a
/// `use` of the second `Dup` beside the item.
#[proc_macro_derive(Clone)]
pub fn not_clone(_item: TokenStream) -> TokenStream {
    "use crate::p_db::Dup;".parse().unwrap()
}
