use indicatif::HumanDuration;
use log::{Level, info, log_enabled};
use std::time::{Duration, Instant};

/// Struct that info logs progress messages on a stream action like encoding.
#[derive(Debug)]
pub struct ProgressLogger {
    target: &'static str,
    start: Instant,
    log_count: u32,
}

impl ProgressLogger {
    pub fn new(target: &'static str, start: Instant) -> Self {
        Self {
            target,
            start,
            log_count: 0,
        }
    }

    /// Update and potentially log progress on a stream action.
    /// * `total` total duration of the stream
    /// * `complete` the duration that has been completed at this time
    /// * `fps` frames per second
    pub fn update(&mut self, total: Duration, completed: Duration, fps: f32) {
        if log_enabled!(Level::Info) && completed > Duration::ZERO {
            let done = completed.as_secs_f64() / total.as_secs_f64();

            let elapsed = self.start.elapsed();

            let before_count = self.log_count;
            while elapsed > self.next_log() {
                self.log_count += 1;
            }
            if before_count == self.log_count {
                return;
            }

            let eta = Duration::from_secs_f64(elapsed.as_secs_f64() / done).saturating_sub(elapsed);
            info!(
                target: self.target,
                "{:.0}%, {fps} fps, eta {}",
                done * 100.0,
                HumanDuration(eta)
            );
        }
    }

    /// First log after >=16s, then >=32s etc
    fn next_log(&self) -> Duration {
        Duration::from_secs(2_u64.pow(self.log_count + 4))
    }
}
