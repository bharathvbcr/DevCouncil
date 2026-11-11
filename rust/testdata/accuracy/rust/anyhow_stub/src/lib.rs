//! A stand-in for `anyhow::bail!` and `anyhow::ensure!`: expression macros that
//! declare nothing.
#[macro_export]
macro_rules! bail {
    ($m:literal) => {
        return Err($m.to_string())
    };
}

#[macro_export]
macro_rules! ensure {
    ($cond:expr, $m:literal) => {
        if !$cond {
            return Err($m.to_string());
        }
    };
}
