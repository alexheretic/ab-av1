use crate::svtav1::SvtArgs;
use anyhow::ensure;
use clap::Parser;
use std::{path::PathBuf, sync::Arc, time::Duration};

/// Common encoding args that apply when using svt-av1.
#[derive(Parser, Clone)]
pub struct SvtEncode {
    /// Input video file.
    #[clap(short, long)]
    pub input: PathBuf,

    /// Encoder preset (0-13). Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long)]
    pub preset: u8,

    /// Interval between keyframes. Can be specified as a number of frames, or a duration.
    /// E.g. "300" or "10s". Longer intervals can give better compression but make seeking
    /// more coarse. Duration will be converted to frames using the input fps.
    #[clap(long)]
    pub keyint: Option<KeyInterval>,

    /// Enable scene change detection. Inserts keyframes at scene changes.
    /// Useful for higher keyframe intervals.
    #[clap(long)]
    pub scd: bool,

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
    pub fn to_svt_args(&self, crf: u8, fps: anyhow::Result<f64>) -> anyhow::Result<SvtArgs<'_>> {
        let args = self
            .args
            .iter()
            .flat_map(|arg| match arg.split_once('=') {
                Some((a, b)) => vec![a, b],
                None => vec![arg.as_ref()],
            })
            .collect();

        Ok(SvtArgs {
            crf,
            input: &self.input,
            preset: self.preset,
            keyint: match self.keyint {
                Some(ki) => Some(ki.svt_keyint(fps)?),
                None => None,
            },
            scd: match self.scd {
                true => 1,
                false => 0,
            },
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
    pub fn svt_keyint(&self, fps: anyhow::Result<f64>) -> anyhow::Result<i32> {
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
