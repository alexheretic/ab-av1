pub mod svtav1;
// pub mod videotoolbox;

use crate::{
    ffmpeg::FfmpegEncodeArgs,
    ffprobe::{Ffprobe, ProbeError},
};
use std::{
    collections::HashMap,
    fmt::{self, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

pub trait Encoder {
    // fn to_encoder_args(&self, probe: &Ffprobe) -> anyhow::Result<FfmpegEncodeArgs<'_>>;

    fn encode_hint(&self) -> String;

    // fn to_ffmpeg_args(&self, probe: &Ffprobe) -> anyhow::Result<FfmpegEncodeArgs<'_>>;

    // fn get_output(&self, dir: Option<PathBuf>, sample: bool) -> PathBuf;

    // fn output(&self) -> PathBuf;

    // fn set_input(&mut self, input: &PathBuf) -> &Self;

    fn keyint(&self, probe: &Ffprobe) -> anyhow::Result<Option<i32>>;
}

/// Video codec for encoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EncoderString(pub Arc<str>);

impl EncoderString {
    /// vcodec name that would work if you used it as the -e argument.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns default crf-increment.
    ///
    /// Generally 0.1 if codec supports decimal crf.
    pub fn default_crf_increment(&self) -> f32 {
        match self.as_str() {
            "libx264" | "libx265" => 0.1,
            _ => 1.0,
        }
    }

    pub fn default_max_crf(&self) -> f32 {
        match self.as_str() {
            "libx264" | "libx265" => 46.0,
            // rav1e: use max -qp
            "librav1e" => 255.0,
            // Works well for svt-av1
            _ => 55.0,
        }
    }

    /// Additional encoder specific ffmpeg arg defaults.
    pub fn default_ffmpeg_args(&self) -> &[(&'static str, &'static str)] {
        match self.as_str() {
            // add `-b:v 0` for aom & vp9 to use "constant quality" mode
            "libaom-av1" | "libvpx-vp9" => &[("-b:v", "0")],
            // enable lookahead mode for qsv encoders
            "av1_qsv" | "hevc_qsv" | "h264_qsv" => &[
                ("-look_ahead", "1"),
                ("-extbrc", "1"),
                ("-look_ahead_depth", "40"),
            ],
            _ => &[],
        }
    }
}

impl std::str::FromStr for EncoderString {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            // Support "svt-av1" alias for back compat
            "svt-av1" => Self("libsvtav1".into()),
            vcodec => Self(vcodec.into()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Preset {
    Number(u8),
    Name(Arc<str>),
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => n.fmt(f),
            Self::Name(name) => name.fmt(f),
        }
    }
}

impl std::str::FromStr for Preset {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.parse::<u8>() {
            Ok(n) => Ok(Self::Number(n)),
            _ => Ok(Self::Name(s.into())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyInterval {
    Frames(i32),
    Duration(Duration),
}

impl KeyInterval {
    pub fn keyint_number(&self, fps: Result<f64, ProbeError>) -> Result<i32, ProbeError> {
        Ok(match self {
            Self::Frames(keyint) => *keyint,
            Self::Duration(duration) => (duration.as_secs_f64() * fps?).round() as i32,
        })
    }
}

impl fmt::Display for KeyInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frames(frames) => write!(f, "{frames}"),
            Self::Duration(d) => write!(f, "{}", humantime::format_duration(*d)),
        }
    }
}

/// Parse as integer frames or a duration.
impl std::str::FromStr for KeyInterval {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let frame_err = match s.parse::<i32>() {
            Ok(f) => return Ok(Self::Frames(f)),
            Err(err) => err,
        };
        match humantime::parse_duration(s) {
            Ok(d) => Ok(Self::Duration(d)),
            Err(e) => Err(anyhow::anyhow!("frames: {frame_err}, duration: {e}")),
        }
    }
}

/// Ordered by ascending quality.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[clap(rename_all = "lower")]
pub enum PixelFormat {
    Yuv420p,
    Yuv420p10le,
    Yuv444p10le,
}

#[test]
fn pixel_format_order() {
    use PixelFormat::*;
    assert!(Yuv420p < Yuv420p10le);
    assert!(Yuv420p10le < Yuv444p10le);
}

impl PixelFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yuv420p10le => "yuv420p10le",
            Self::Yuv444p10le => "yuv444p10le",
            Self::Yuv420p => "yuv420p",
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl TryFrom<&str> for PixelFormat {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "yuv420p10le" => Ok(Self::Yuv420p10le),
            "yuv444p10le" => Ok(Self::Yuv444p10le),
            "yuv420p" => Ok(Self::Yuv420p),
            _ => Err(()),
        }
    }
}

fn try_parse_fps_vfilter(vfilter: &str) -> Option<f64> {
    let fps_filter = vfilter
        .split(',')
        .find_map(|vf| vf.trim().strip_prefix("fps="))?
        .trim();

    match fps_filter {
        "ntsc" => Some(30000.0 / 1001.0),
        "pal" => Some(25.0),
        "film" => Some(24.0),
        "ntsc_film" => Some(24000.0 / 1001.0),
        _ => crate::ffprobe::parse_frame_rate(fps_filter),
    }
}

mod test {
    use super::*;

    #[test]
    fn test_try_parse_fps_vfilter() {
        let fps = try_parse_fps_vfilter("scale=1280:-1, fps=24, transpose=1").unwrap();
        assert!((fps - 24.0).abs() < f64::EPSILON, "{fps:?}");

        let fps = try_parse_fps_vfilter("scale=1280:-1, fps=ntsc, transpose=1").unwrap();
        assert!((fps - 30000.0 / 1001.0).abs() < f64::EPSILON, "{fps:?}");
    }

    #[test]
    fn frame_interval_from_str() {
        use std::str::FromStr;
        let from_300 = KeyInterval::from_str("300").unwrap();
        assert_eq!(from_300, KeyInterval::Frames(300));
    }

    #[test]
    fn duration_interval_from_str() {
        use std::{str::FromStr, time::Duration};
        let from_10s = KeyInterval::from_str("10s").unwrap();
        assert_eq!(from_10s, KeyInterval::Duration(Duration::from_secs(10)));
    }
}
