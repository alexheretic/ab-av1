//! Shared argument logic.
mod encode;
mod vmaf;

pub use encode::*;
pub use vmaf::*;

use crate::{command::encode::default_output_ext, ffprobe::Ffprobe};
use clap::{Parser, ValueHint};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Encoding args that apply when encoding to an output.
#[derive(Parser, Clone)]
pub struct EncodeToOutput {
    /// Output file, by default the same as input with `.av1` before the extension.
    ///
    /// E.g. if unspecified: -i vid.mkv --> vid.av1.mkv
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    pub output: Option<PathBuf>,

    /// Set the output ffmpeg audio codec.
    /// By default 'copy' is used. Otherwise, if re-encoding is necessary, 'libopus' is default.
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

    /// Only process the main video stream, drop all other streams.
    ///
    /// The output will be a single video stream.
    #[arg(long)]
    pub video_only: bool,

    /// By default input files will not be overwritten to prevent accidental data loss.
    /// Setting this option overrides that allowing input overwrites.
    #[arg(long)]
    pub overwrite_input: bool,
}

/// Sampling arguments.
#[derive(Parser, Clone)]
pub struct Sample {
    /// Number of samples to use across the input video. Overrides --sample-every.
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

    /// Minimum number of samples. So at least this many samples will be used.
    #[arg(long)]
    pub min_samples: Option<u64>,

    /// Duration of each sample.
    #[arg(long, default_value = "20s", value_parser = humantime::parse_duration)]
    pub sample_duration: Duration,

    /// Keep temporary files after exiting.
    #[arg(long)]
    pub keep: bool,

    /// Directory to store temporary sample data in.
    /// Defaults to using the input's directory.
    #[arg(long, env = "AB_AV1_TEMP_DIR", value_hint = ValueHint::DirPath)]
    pub temp_dir: Option<PathBuf>,

    /// Extension preference for encoded samples (ffmpeg encoder only).
    #[arg(skip)]
    pub extension: Option<Arc<str>>,
}

impl Sample {
    /// Calculate the desired sample count using `samples` or `sample_every` & `min_samples`.
    pub fn sample_count(&self, input_duration: Duration) -> u64 {
        match self.samples {
            Some(s) => s,
            None => {
                (input_duration.as_secs_f64() / self.sample_every.as_secs_f64().max(1.0)).ceil()
                    as _
            }
        }
        .max(self.min_samples.unwrap_or(1))
        .max(1)
    }

    pub fn set_extension_from_input(&mut self, input: &Path, encoder: &Encoder, probe: &Ffprobe) {
        self.extension = Some(default_output_ext(input, encoder, probe.is_image).into());
    }

    pub fn set_extension_from_output(&mut self, output: &Path) {
        self.extension = output.extension().and_then(|e| e.to_str().map(Into::into));
    }
}

/// Args for when VMAF/XPSNR are used to score ref vs distorted.
#[derive(Debug, Parser, Clone, Hash)]
pub struct ScoreArgs {
    /// Ffmpeg video filter applied to the VMAF/XPSNR reference before analysis.
    /// E.g. --reference-vfilter "scale=1280:-1,fps=24".
    ///
    /// Overrides --vfilter which would otherwise be used.
    #[arg(long)]
    pub reference_vfilter: Option<Arc<str>>,
}

/// Common xpsnr options.
#[derive(Debug, Parser, Clone, Copy)]
pub struct Xpsnr {
    /// Frame rate override used to analyse both reference & distorted videos.
    /// Maps to ffmpeg `-r` input arg.
    ///
    /// Setting to 0 disables use.
    #[arg(long, default_value_t = 60.0)]
    pub xpsnr_fps: f32,
}

impl Xpsnr {
    pub fn fps(&self) -> Option<f32> {
        Some(self.xpsnr_fps).filter(|r| *r > 0.0)
    }
}

impl std::hash::Hash for Xpsnr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.xpsnr_fps.to_ne_bytes().hash(state);
    }
}
