pub mod args;
pub mod auto_encode;
pub mod crf_search;
pub mod encode;
pub mod encoders;
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

/// Helper trait for durations under 584942 years or so.
trait SmallDuration {
    /// Returns the total number of whole microseconds.
    fn as_micros_u64(&self) -> u64;
}

impl SmallDuration for std::time::Duration {
    fn as_micros_u64(&self) -> u64 {
        self.as_micros().try_into().unwrap_or(u64::MAX)
    }
}
