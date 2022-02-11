pub mod args;
pub mod auto_encode;
pub mod crf_search;
pub mod encode;
pub mod print_completions;
pub mod sample_encode;
pub mod vmaf;

pub use auto_encode::auto_encode;
pub use crf_search::crf_search;
pub use encode::encode;
pub use print_completions::print_completions;
pub use sample_encode::sample_encode;
pub use vmaf::vmaf;

const PROGRESS_CHARS: &str = "##-";
