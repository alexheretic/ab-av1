use crate::{
    ffprobe::{Ffprobe, ProbeError},
    svtav1::SvtArgs,
};
use anyhow::ensure;
use clap::Parser;
use std::{
    fmt::{self, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

/// Common svt-av1 input encoding arguments.
#[derive(Parser, Clone)]
pub struct SvtEncode {
    /// Input video file.
    #[clap(short, long, value_parser)]
    pub input: PathBuf,

    /// Ffmpeg video filter applied to the input before av1 encoding.
    /// E.g. --vfilter "scale=1280:-1,fps=24".
    ///
    /// See https://ffmpeg.org/ffmpeg-filters.html#Video-Filters
    #[clap(long, value_parser)]
    pub vfilter: Option<String>,

    /// Pixel format.
    #[clap(arg_enum, long, value_parser, default_value_t = PixelFormat::Yuv420p10le)]
    pub pix_format: PixelFormat,

    /// Encoder preset (0-13). Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long, value_parser)]
    pub preset: u8,

    /// Interval between keyframes. Can be specified as a number of frames, or a duration.
    /// E.g. "300" or "10s". Defaults to 10s if the input duration is over 3m.
    ///
    /// Longer intervals can give better compression but make seeking more coarse.
    /// Durations will be converted to frames using the input fps.
    #[clap(long, value_parser)]
    pub keyint: Option<KeyInterval>,

    /// Scene change detection, inserts keyframes at scene changes.
    /// Defaults on if using default keyint & the input duration is over 3m. Otherwise off.
    #[clap(long, value_parser)]
    pub scd: Option<bool>,

    /// Additional svt-av1 arg(s). E.g. --svt mbr=2000 --svt film-grain=30
    ///
    /// See https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/svt-av1_encoder_user_guide.md#options
    #[clap(long = "svt", value_parser = parse_svt_arg)]
    pub args: Vec<Arc<str>>,
}

fn parse_svt_arg(arg: &str) -> anyhow::Result<Arc<str>> {
    let mut arg = arg.to_owned();
    if !arg.starts_with('-') {
        if arg.find('=') == Some(1) || arg.len() == 1 {
            arg.insert(0, '-');
        } else {
            arg.insert_str(0, "--");
        }
    }

    for deny in [
        "-i",
        "-b",
        "--crf",
        "--preset",
        "--keyint",
        "--scd",
        "--input-depth",
    ] {
        ensure!(!arg.starts_with(deny), "svt arg {deny} cannot be used here");
    }

    Ok(arg.into())
}

impl SvtEncode {
    pub fn to_svt_args(&self, crf: u8, probe: &Ffprobe) -> Result<SvtArgs<'_>, ProbeError> {
        const KEYINT_DEFAULT_INPUT_MIN: Duration = Duration::from_secs(60 * 3);
        const KEYINT_DEFAULT: Duration = Duration::from_secs(10);

        let args = self
            .args
            .iter()
            .flat_map(|arg| match arg.split_once('=') {
                Some((a, b)) => vec![a, b],
                None => vec![arg.as_ref()],
            })
            .collect();

        let filter_fps = self.vfilter.as_deref().and_then(try_parse_fps_vfilter);
        let keyint = match (self.keyint, &probe.duration, &probe.fps, filter_fps) {
            // use the filter-fps if used, otherwise the input fps
            (Some(ki), .., Some(fps)) => Some(ki.svt_keyint(Ok(fps))?),
            (Some(ki), _, fps, None) => Some(ki.svt_keyint(fps.clone())?),
            (None, Ok(duration), _, Some(fps)) if *duration >= KEYINT_DEFAULT_INPUT_MIN => {
                Some(KeyInterval::Duration(KEYINT_DEFAULT).svt_keyint(Ok(fps))?)
            }
            (None, Ok(duration), Ok(fps), None) if *duration >= KEYINT_DEFAULT_INPUT_MIN => {
                Some(KeyInterval::Duration(KEYINT_DEFAULT).svt_keyint(Ok(*fps))?)
            }
            _ => None,
        };
        let scd = match (self.scd, self.keyint, keyint) {
            (Some(true), ..) | (_, None, Some(_)) => 1,
            _ => 0,
        };

        Ok(SvtArgs {
            input: &self.input,
            pix_fmt: self.pix_format,
            vfilter: self.vfilter.as_deref(),
            crf,
            preset: self.preset,
            keyint,
            scd,
            args,
        })
    }

    pub fn encode_hint(&self, crf: u8) -> String {
        let Self {
            input,
            vfilter,
            preset,
            pix_format,
            keyint,
            scd,
            args,
        } = self;

        let mut hint = format!("ab-av1 encode -i {input:?} --crf {crf} --preset {preset}");
        if let Some(keyint) = keyint {
            write!(hint, " --keyint {keyint}").unwrap();
        }
        if let Some(scd) = scd {
            write!(hint, " --scd {scd}").unwrap();
        }
        if *pix_format != PixelFormat::Yuv420p10le {
            write!(hint, " --pix-format {pix_format}").unwrap();
        }
        if let Some(filter) = vfilter {
            write!(hint, " --vfilter {filter:?}").unwrap();
        }
        for arg in args {
            let arg = arg.trim_start_matches('-');
            write!(hint, " --svt {arg}").unwrap();
        }

        hint
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyInterval {
    Frames(i32),
    Duration(Duration),
}

impl KeyInterval {
    pub fn svt_keyint(&self, fps: Result<f64, ProbeError>) -> Result<i32, ProbeError> {
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

#[derive(clap::ArgEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[clap(rename_all = "lower")]
pub enum PixelFormat {
    Yuv420p10le,
    Yuv420p,
}

impl PixelFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yuv420p10le => "yuv420p10le",
            Self::Yuv420p => "yuv420p",
        }
    }

    pub fn input_depth(self) -> &'static str {
        match self {
            Self::Yuv420p10le => "10",
            Self::Yuv420p => "8",
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
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

#[test]
fn test_try_parse_fps_vfilter() {
    let fps = try_parse_fps_vfilter("scale=1280:-1, fps=24, transpose=1").unwrap();
    assert!((fps - 24.0).abs() < f64::EPSILON, "{:?}", fps);

    let fps = try_parse_fps_vfilter("scale=1280:-1, fps=ntsc, transpose=1").unwrap();
    assert!((fps - 30000.0 / 1001.0).abs() < f64::EPSILON, "{:?}", fps);
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

/// Should use keyint & scd defaults for >3m inputs.
#[test]
fn to_svt_args_default_over_3m() {
    let svt = SvtEncode {
        input: "vid.mp4".into(),
        vfilter: Some("scale=320:-1,fps=film".into()),
        preset: 8,
        pix_format: PixelFormat::Yuv420p10le,
        keyint: None,
        scd: None,
        args: vec!["film-grain=30".into()],
    };

    let probe = Ffprobe {
        duration: Ok(Duration::from_secs(300)),
        has_audio: true,
        max_audio_channels: None,
        fps: Ok(30.0),
        resolution: Some((1280, 720)),
    };

    let SvtArgs {
        input,
        vfilter,
        pix_fmt,
        crf,
        preset,
        keyint,
        scd,
        args,
    } = svt.to_svt_args(32, &probe).expect("to_svt_args");

    assert_eq!(input, svt.input);
    assert_eq!(vfilter, Some("scale=320:-1,fps=film"));
    assert_eq!(crf, 32);
    assert_eq!(preset, svt.preset);
    assert_eq!(pix_fmt, svt.pix_format);
    assert_eq!(keyint, Some(240)); // based off filter fps
    assert_eq!(scd, 1);
    assert_eq!(args, vec!["film-grain", "30"]);
}

#[test]
fn to_svt_args_default_under_3m() {
    let svt = SvtEncode {
        input: "vid.mp4".into(),
        vfilter: None,
        preset: 8,
        pix_format: PixelFormat::Yuv420p,
        keyint: None,
        scd: None,
        args: vec![],
    };

    let probe = Ffprobe {
        duration: Ok(Duration::from_secs(179)),
        has_audio: true,
        max_audio_channels: None,
        fps: Ok(24.0),
        resolution: Some((1280, 720)),
    };

    let SvtArgs {
        input,
        vfilter,
        pix_fmt,
        crf,
        preset,
        keyint,
        scd,
        args,
    } = svt.to_svt_args(32, &probe).expect("to_svt_args");

    assert_eq!(input, svt.input);
    assert_eq!(vfilter, None);
    assert_eq!(crf, 32);
    assert_eq!(preset, svt.preset);
    assert_eq!(pix_fmt, svt.pix_format);
    assert_eq!(keyint, None);
    assert_eq!(scd, 0);
    assert!(args.is_empty());
}
