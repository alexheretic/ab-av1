use anyhow::ensure;
use indicatif::HumanDuration;
use log::{Level, info, log_enabled};
use std::{
    fmt,
    time::{Duration, Instant},
};

/// Interval between progress log messages when running non-interactively.
#[derive(Debug, Clone, Copy)]
pub enum LogInterval {
    /// Fixed duration between log messages.
    Duration(Duration),
    /// Fixed percentage of total progress between log messages.
    Percent(f32),
}

impl std::str::FromStr for LogInterval {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        if let Some(pct) = s.strip_suffix('%') {
            let val: f32 = pct
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid percentage: {s}"))?;
            ensure!(
                val > 0.0 && val <= 100.0,
                "percentage must be between 0 and 100"
            );
            return Ok(Self::Percent(val));
        }
        if let Ok(d) = humantime::parse_duration(s) {
            ensure!(!d.is_zero(), "interval must be greater than 0");
            return Ok(Self::Duration(d));
        }
        anyhow::bail!(
            "invalid interval '{s}': expected duration (e.g., '30s', '1m') or percentage (e.g., '5%')"
        );
    }
}

impl fmt::Display for LogInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duration(d) => write!(f, "{}", humantime::format_duration(*d)),
            Self::Percent(p) => write!(f, "{p}%"),
        }
    }
}

/// Struct that info logs progress messages on a stream action like encoding.
#[derive(Debug)]
pub struct ProgressLogger {
    target: &'static str,
    start: Instant,
    interval: Option<LogInterval>,
    log_count: u32,
    last_log_percent: f32,
}

impl ProgressLogger {
    pub fn new(target: &'static str, start: Instant, interval: Option<LogInterval>) -> Self {
        Self {
            target,
            start,
            interval,
            log_count: 0,
            last_log_percent: 0.0,
        }
    }

    /// Update and potentially log progress on a stream action.
    /// * `total` total duration of the stream
    /// * `complete` the duration that has been completed at this time
    /// * `fps` frames per second
    pub fn update(&mut self, total: Duration, completed: Duration, fps: f32) {
        if log_enabled!(Level::Info) && completed > Duration::ZERO {
            let done = completed.as_secs_f64() / total.as_secs_f64();

            if !self.should_log(done) {
                return;
            }

            let elapsed = self.start.elapsed();
            let eta = Duration::from_secs_f64(elapsed.as_secs_f64() / done).saturating_sub(elapsed);
            info!(
                target: self.target,
                "{:.0}%, {fps} fps, eta {}",
                done * 100.0,
                HumanDuration(eta)
            );
        }
    }

    fn should_log(&mut self, done: f64) -> bool {
        match self.interval {
            None => self.should_log_exponential(),
            Some(LogInterval::Duration(interval)) => self.should_log_duration(interval),
            Some(LogInterval::Percent(pct)) => self.should_log_percent(done, pct),
        }
    }

    fn should_log_exponential(&mut self) -> bool {
        let elapsed = self.start.elapsed();
        let before_count = self.log_count;
        while elapsed > self.next_log_exponential() {
            self.log_count += 1;
        }
        before_count != self.log_count
    }

    fn should_log_duration(&mut self, interval: Duration) -> bool {
        let elapsed = self.start.elapsed();
        let expected_count = (elapsed.as_secs_f64() / interval.as_secs_f64()) as u32;
        if expected_count > self.log_count {
            self.log_count = expected_count;
            true
        } else {
            false
        }
    }

    fn should_log_percent(&mut self, done: f64, interval_pct: f32) -> bool {
        let current_pct = (done * 100.0) as f32;
        if current_pct >= self.last_log_percent + interval_pct {
            self.last_log_percent = (current_pct / interval_pct).floor() * interval_pct;
            true
        } else {
            false
        }
    }

    /// First log after >=16s, then >=32s etc
    fn next_log_exponential(&self) -> Duration {
        Duration::from_secs(2_u64.pow(self.log_count + 4))
    }
}
