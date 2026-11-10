//! An attribute macro that emits a `use` beside the item it decorates.
use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn with_use(args: TokenStream, item: TokenStream) -> TokenStream {
    format!("use {}; {}", args, item).parse().unwrap()
}
