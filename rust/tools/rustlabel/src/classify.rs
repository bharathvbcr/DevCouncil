//! The two classes the Rust recall floors are set on, `free_self` and
//! `non_self` (`corpora.json`: `rust_free_self`, `rust_non_self`).
//!
//! `free_self` is a site whose callee needs no knowledge of another value's
//! type: a bare or module-qualified call to a free function, and anything
//! reached through the enclosing type itself (`self.m()`, `Self::f()`).
//! `non_self` is a site that does: a method call on any receiver but `self`, a
//! path to an associated function or trait method of a named type
//! (`Counter::new()`, `Speak::speak(&d)`), and an operator, which dispatches on
//! its operand's type.
//!
//! The class is a function of how the site is written and of whether the
//! oracle's target is a method. It never reads what DevMap resolved.

/// How a site is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteShape {
    /// `f(..)`.
    BareCall,
    /// `a::b::f(..)`; `qualifier` is the written path before the last segment.
    PathCall { qualifier: String },
    /// `receiver.m(..)`; `receiver` is the receiver expression's source text.
    MethodCall { receiver: String },
    /// An operator, `?` or an index, which call an operator trait's method.
    Operator,
    /// A function used as a value: `map(f)`, `Self::step`, `util::go`.
    ValuePath { qualifier: Option<String> },
}

/// The reporting class a labels file carries in `construct`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Construct {
    FreeSelf,
    NonSelf,
}

impl Construct {
    /// The spelling the corpus runner keys on.
    pub fn as_str(self) -> &'static str {
        match self {
            Construct::FreeSelf => "free_self",
            Construct::NonSelf => "non_self",
        }
    }
}

/// Whether a path's first segment names the enclosing type or module.
fn names_self(qualifier: &str) -> bool {
    matches!(qualifier.trim(), "Self" | "self")
}

fn by_path(qualifier: &str, target_is_method: bool) -> Construct {
    if names_self(qualifier) || !target_is_method {
        Construct::FreeSelf
    } else {
        Construct::NonSelf
    }
}

/// The class of a site written as `shape` whose oracle target is a method
/// (`target_is_method`) or a free function.
pub fn classify(shape: &SiteShape, target_is_method: bool) -> Construct {
    match shape {
        SiteShape::BareCall => Construct::FreeSelf,
        SiteShape::MethodCall { receiver } => {
            if receiver.trim() == "self" {
                Construct::FreeSelf
            } else {
                Construct::NonSelf
            }
        }
        SiteShape::PathCall { qualifier } => by_path(qualifier, target_is_method),
        SiteShape::ValuePath { qualifier: None } => Construct::FreeSelf,
        SiteShape::ValuePath {
            qualifier: Some(qualifier),
        } => by_path(qualifier, target_is_method),
        SiteShape::Operator => Construct::NonSelf,
    }
}
