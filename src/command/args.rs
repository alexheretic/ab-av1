//! Shared argument logic.
mod encode;
mod vmaf;

pub use encode::*;
pub use vmaf::*;

use clap::Parser;
use std::{path::PathBuf, time::Duration};

/// Encoding args that apply when encoding to an output.
#[derive(Parser, Clone)]
pub struct EncodeToOutput {
    /// Output file, by default the same as input with `.av1` before the extension.
    ///
    /// E.g. if unspecified: -i vid.mp4 --> vid.av1.mp4
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Set the output ffmpeg audio codec.
    /// By default when the input & output file extension match 'copy' is used,
    /// otherwise 'libopus'.
    ///
    /// See https://ffmpeg.org/ffmpeg.html#Audio-Options.
    #[arg(long = "acodec")]
    pub audio_codec: Option<String>,

    /// Downmix input audio streams to stereo if input streams use greater than
    /// 3 channels.
    ///
    /// No effect if the input audio has 3 or fewer channels.
    #[arg(long)]
    pub downmix_to_stereo: bool,
}

/// Sampling arguments.
#[derive(Parser, Clone)]
pub struct Sample {
    /// Number of 20s samples to use across the input video. Overrides --sample-every.
    /// More samples take longer but may provide a more accurate result.
    #[arg(long)]
    pub samples: Option<u64>,

    /// Calculate number of samples by dividing the input duration by this value.
    /// So "12m" would mean with an input 25-36 minutes long, 3 samples would be used.
    /// More samples take longer but may provide a more accurate result.
    ///
    /// Setting --samples overrides this value.
    #[arg(long, default_value = "12m", value_parser = humantime::parse_duration)]
    pub sample_every: Duration,

    /// Directory to store temporary sample data in.
    /// Defaults to using the input's directory.
    #[arg(long, env = "AB_AV1_TEMP_DIR")]
    pub temp_dir: Option<PathBuf>,
}

impl Sample {
    /// Calculate the desired sample count using `samples` or `sample_every`.
    pub fn sample_count(&self, input_duration: Duration) -> u64 {
        if let Some(s) = self.samples {
            return s;
        }
        (input_duration.as_secs_f64() / self.sample_every.as_secs_f64().max(1.0)).ceil() as _
    }
}
