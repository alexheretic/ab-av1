macro_rules! style {
    ($($x:tt)*) => {
        console::style(format!($($x)*))
    }
}
pub(crate) use style;
