//! A stand-in for `anyhow::bail!`: an expression macro that declares nothing.
#[macro_export]
macro_rules! bail {
    ($m:literal) => {
        return Err($m.to_string())
    };
}
