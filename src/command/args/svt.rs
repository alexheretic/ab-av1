use crate::{
    ffprobe::{Ffprobe, ProbeError},
    svtav1::SvtArgs,
};
use anyhow::ensure;
use clap::Parser;
use std::{fmt, path::PathBuf, sync::Arc, time::Duration};

/// Common encoding args that apply when using svt-av1.
#[derive(Parser, Clone)]
pub struct SvtEncode {
    /// Input video file.
    #[clap(short, long)]
    pub input: PathBuf,

    /// Pixel format.
    #[clap(arg_enum, long, default_value_t = PixelFormat::Yuv420p10le)]
    pub pix_format: PixelFormat,

    /// Encoder preset (0-13). Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long)]
    pub preset: u8,

    /// Interval between keyframes. Can be specified as a number of frames, or a duration.
    /// E.g. "300" or "10s". Defaults to 10s if the input duration is over 3m.
    ///
    /// Longer intervals can give better compression but make seeking more coarse.
    /// Durations will be converted to frames using the input fps.
    #[clap(long)]
    pub keyint: Option<KeyInterval>,

    /// Scene change detection, inserts keyframes at scene changes.
    /// Defaults on if using default keyint & the input duration is over 3m. Otherwise off.
    #[clap(long)]
    pub scd: Option<bool>,

    /// Additional svt-av1 arg(s). E.g. --svt mbr=2000 --svt film-grain=30
    ///
    /// See https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/svt-av1_encoder_user_guide.md#options
    #[clap(long = "svt", parse(try_from_str = parse_svt_arg))]
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

        let keyint = match (self.keyint, &probe.duration, &probe.fps) {
            (Some(ki), _, fps) => Some(ki.svt_keyint(fps.clone())?),
            (None, Ok(duration), Ok(fps)) if *duration >= KEYINT_DEFAULT_INPUT_MIN => {
                Some(KeyInterval::Duration(KEYINT_DEFAULT).svt_keyint(Ok(*fps))?)
            }
            _ => None,
        };
        let scd = match (self.scd, self.keyint, &probe.duration, &probe.fps) {
            (Some(true), ..) => 1,
            (None, None, Ok(duration), Ok(_)) if *duration >= KEYINT_DEFAULT_INPUT_MIN => 1,
            _ => 0,
        };

        Ok(SvtArgs {
            crf,
            input: &self.input,
            preset: self.preset,
            pix_fmt: self.pix_format,
            keyint,
            scd,
            args,
        })
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
        preset: 8,
        pix_format: PixelFormat::Yuv420p10le,
        keyint: None,
        scd: None,
        args: vec!["film-grain=30".into()],
    };

    let probe = Ffprobe {
        duration: Ok(Duration::from_secs(300)),
        has_audio: true,
        fps: Ok(24.0),
    };

    let SvtArgs {
        input,
        crf,
        preset,
        pix_fmt,
        keyint,
        scd,
        args,
    } = svt.to_svt_args(32, &probe).expect("to_svt_args");

    assert_eq!(input, svt.input);
    assert_eq!(crf, 32);
    assert_eq!(preset, svt.preset);
    assert_eq!(pix_fmt, svt.pix_format);
    assert_eq!(keyint, Some(240));
    assert_eq!(scd, 1);
    assert_eq!(args, vec!["film-grain", "30"]);
}

#[test]
fn to_svt_args_default_under_3m() {
    let svt = SvtEncode {
        input: "vid.mp4".into(),
        preset: 8,
        pix_format: PixelFormat::Yuv420p,
        keyint: None,
        scd: None,
        args: vec![],
    };

    let probe = Ffprobe {
        duration: Ok(Duration::from_secs(179)),
        has_audio: true,
        fps: Ok(24.0),
    };

    let SvtArgs {
        input,
        crf,
        preset,
        pix_fmt,
        keyint,
        scd,
        args,
    } = svt.to_svt_args(32, &probe).expect("to_svt_args");

    assert_eq!(input, svt.input);
    assert_eq!(crf, 32);
    assert_eq!(preset, svt.preset);
    assert_eq!(pix_fmt, svt.pix_format);
    assert_eq!(keyint, None);
    assert_eq!(scd, 0);
    assert!(args.is_empty());
}
