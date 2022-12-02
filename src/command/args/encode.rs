use crate::{
    ffmpeg::FfmpegEncodeArgs,
    ffprobe::{Ffprobe, ProbeError},
    float::TerseF32,
    svtav1::SvtArgs,
};
use anyhow::ensure;
use clap::Parser;
use std::{
    collections::HashMap,
    fmt::{self, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

/// Common svt-av1/ffmpeg input encoding arguments.
#[derive(Parser, Clone)]
pub struct Encode {
    /// Encoder override. See https://ffmpeg.org/ffmpeg-all.html#toc-Video-Encoders.
    ///
    /// [possible values: svt-av1, libx264, libx265, libvpx-vp9, ...]
    #[arg(value_enum, short, long, default_value = "svt-av1")]
    pub encoder: Encoder,

    /// Input video file.
    #[arg(short, long)]
    pub input: PathBuf,

    /// Ffmpeg video filter applied to the input before av1 encoding.
    /// E.g. --vfilter "scale=1280:-1,fps=24".
    ///
    /// See https://ffmpeg.org/ffmpeg-filters.html#Video-Filters
    #[arg(long)]
    pub vfilter: Option<String>,

    /// Pixel format. svt-av1 default yuv420p10le.
    #[arg(value_enum, long)]
    pub pix_format: Option<PixelFormat>,

    /// Encoder preset (0-13).
    /// Higher presets means faster encodes, but with a quality tradeoff.
    ///
    /// For some ffmpeg encodes a word may be used, e.g. "fast".
    /// libaom-av1 preset is mapped to equivalent -cpu-used argument.
    ///
    /// [svt-av1 default: 8]
    #[arg(long)]
    pub preset: Option<Preset>,

    /// Interval between keyframes. Can be specified as a number of frames, or a duration.
    /// E.g. "300" or "10s". Defaults to 10s if the input duration is over 3m.
    ///
    /// Longer intervals can give better compression but make seeking more coarse.
    /// Durations will be converted to frames using the input fps.
    ///
    /// Works on svt-av1 & most ffmpeg encoders set with --encoder.
    #[arg(long)]
    pub keyint: Option<KeyInterval>,

    /// Svt-av1 scene change detection, inserts keyframes at scene changes.
    /// Defaults on if using default keyint & the input duration is over 3m. Otherwise off.
    #[arg(long)]
    pub scd: Option<bool>,

    /// Additional svt-av1 arg(s). E.g. --svt mbr=2000 --svt film-grain=30
    ///
    /// See https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/svt-av1_encoder_user_guide.md#options
    #[arg(long = "svt", value_parser = parse_svt_arg)]
    pub svt_args: Vec<Arc<str>>,

    /// Additional ffmpeg encoder arg(s). E.g. `--enc x265-params=lossless=1`
    /// These are added as ffmpeg output file options.
    ///
    /// The first '=' symbol will be used to infer that this is an option with a value.
    /// Passed to ffmpeg like "x265-params=lossless=1" -> ['-x265-params', 'lossless=1']
    #[arg(long = "enc", allow_hyphen_values = true, value_parser = parse_enc_arg)]
    pub enc_args: Vec<String>,

    /// Additional ffmpeg input encoder arg(s). E.g. `--enc-input r=1`
    /// These are added as ffmpeg input file options.
    ///
    /// See --enc docs.
    #[arg(long = "enc-input", allow_hyphen_values = true, value_parser = parse_enc_arg)]
    pub enc_input_args: Vec<String>,
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

fn parse_enc_arg(arg: &str) -> anyhow::Result<String> {
    let mut arg = arg.to_owned();
    if !arg.starts_with('-') {
        arg.insert(0, '-');
    }
    Ok(arg)
}

impl Encode {
    pub fn to_encoder_args(&self, crf: f32, probe: &Ffprobe) -> anyhow::Result<EncoderArgs<'_>> {
        match self.encoder {
            Encoder::SvtAv1 => Ok(EncoderArgs::SvtAv1(self.to_svt_args(crf, probe)?)),
            Encoder::Ffmpeg(ref vcodec) => Ok(EncoderArgs::Ffmpeg(self.to_ffmpeg_args(
                Arc::clone(vcodec),
                crf,
                probe,
            )?)),
        }
    }

    pub fn encode_hint(&self, crf: f32) -> String {
        let Self {
            encoder,
            input,
            vfilter,
            preset,
            pix_format,
            keyint,
            scd,
            svt_args,
            enc_args,
            enc_input_args,
        } = self;

        let input = shell_escape::escape(input.display().to_string().into());

        let mut hint = "ab-av1 encode".to_owned();

        if let Encoder::Ffmpeg(vcodec) = encoder {
            write!(hint, " -e {vcodec}").unwrap();
        }
        write!(hint, " -i {input} --crf {}", TerseF32(crf)).unwrap();

        if let Some(preset) = preset {
            write!(hint, " --preset {preset}").unwrap();
        }
        if let Some(keyint) = keyint {
            write!(hint, " --keyint {keyint}").unwrap();
        }
        if let Some(scd) = scd {
            write!(hint, " --scd {scd}").unwrap();
        }
        if let Some(pix_fmt) = pix_format {
            write!(hint, " --pix-format {pix_fmt}").unwrap();
        }
        if let Some(filter) = vfilter {
            write!(hint, " --vfilter {filter:?}").unwrap();
        }
        for arg in svt_args {
            let arg = arg.trim_start_matches('-');
            write!(hint, " --svt {arg}").unwrap();
        }
        for arg in enc_input_args {
            let arg = arg.trim_start_matches('-');
            write!(hint, " --enc-input {arg}").unwrap();
        }
        for arg in enc_args {
            let arg = arg.trim_start_matches('-');
            write!(hint, " --enc {arg}").unwrap();
        }

        hint
    }

    fn to_svt_args(&self, crf: f32, probe: &Ffprobe) -> anyhow::Result<SvtArgs<'_>> {
        ensure!(
            self.enc_args.is_empty(),
            "--enc args cannot be used with svt-av1, instead use --svt"
        );
        ensure!(
            self.enc_input_args.is_empty(),
            "--enc-input args cannot be used with svt-av1, instead use --svt"
        );

        let preset = match &self.preset {
            Some(Preset::Number(n)) => *n,
            Some(Preset::Name(n)) => {
                anyhow::bail!("Invalid svt-av1 --preset must be a number not `{n}`")
            }
            None => 8,
        };

        let args = self
            .svt_args
            .iter()
            .flat_map(|arg| match arg.split_once('=') {
                Some((a, b)) => vec![a, b],
                None => vec![arg.as_ref()],
            })
            .collect();

        let keyint = self.keyint(probe)?;
        let scd = match (self.scd, self.keyint, keyint) {
            (Some(true), ..) | (_, None, Some(_)) => 1,
            _ => 0,
        };

        Ok(SvtArgs {
            input: &self.input,
            pix_fmt: self.pix_format.unwrap_or(PixelFormat::Yuv420p10le),
            vfilter: self.vfilter.as_deref(),
            crf,
            preset,
            keyint,
            scd,
            args,
        })
    }

    fn to_ffmpeg_args(
        &self,
        vcodec: Arc<str>,
        crf: f32,
        probe: &Ffprobe,
    ) -> anyhow::Result<FfmpegEncodeArgs<'_>> {
        ensure!(
            self.svt_args.is_empty(),
            "--svt args cannot be used with other encoders, instead use --enc"
        );

        let preset = match &self.preset {
            Some(Preset::Number(n)) => Some(n.to_string().into()),
            Some(Preset::Name(n)) => Some(n.clone()),
            None => None,
        };

        let mut args: Vec<Arc<String>> = self
            .enc_args
            .iter()
            .flat_map(|arg| {
                if let Some((opt, val)) = arg.split_once('=') {
                    vec![opt.to_owned().into(), val.to_owned().into()].into_iter()
                } else {
                    vec![arg.clone().into()].into_iter()
                }
            })
            .collect();

        // Set keyint/-g for all vcodecs
        if !args.iter().any(|a| &**a == "-g") {
            if let Some(keyint) = self.keyint(probe)? {
                args.push("-g".to_owned().into());
                args.push(keyint.to_string().into());
            }
        }

        // add `-b:v 0` for aom & vp9 to use "constant quality" mode
        if matches!(&*vcodec, "libaom-av1" | "libvpx-vp9")
            && !args.iter().any(|arg| &**arg == "-b:v")
        {
            args.push("-b:v".to_owned().into());
            args.push("0".to_owned().into());
        }

        let pix_fmt = self.pix_format.unwrap_or(match &*vcodec {
            "libaom-av1" => PixelFormat::Yuv420p10le,
            _ => PixelFormat::Yuv420p,
        });

        let input_args: Vec<Arc<String>> = self
            .enc_input_args
            .iter()
            .flat_map(|arg| {
                if let Some((opt, val)) = arg.split_once('=') {
                    vec![opt.to_owned().into(), val.to_owned().into()].into_iter()
                } else {
                    vec![arg.clone().into()].into_iter()
                }
            })
            .collect();

        // ban usage of the bits we already set via other args & logic
        let reserved = HashMap::from([
            ("-c:a", " use --acodec"),
            ("-codec:a", " use --acodec"),
            ("-acodec", " use --acodec"),
            ("-i", ""),
            ("-y", ""),
            ("-n", ""),
            ("-c:v", ""),
            ("-codec:v", ""),
            ("-vcodec", ""),
            ("-pix_fmt", " use --pix-format"),
            ("-crf", ""),
            ("-preset", " use --preset"),
            ("-vf", " use --vfilter"),
            ("-filter:v", " use --vfilter"),
        ]);
        for arg in args.iter().chain(input_args.iter()) {
            if let Some(hint) = reserved.get(arg.as_str()) {
                anyhow::bail!("Encoder argument `{arg}` not allowed{hint}");
            }
        }

        Ok(FfmpegEncodeArgs {
            input: &self.input,
            vcodec,
            pix_fmt,
            vfilter: self.vfilter.as_deref(),
            crf,
            preset,
            output_args: args,
            input_args,
        })
    }

    fn keyint(&self, probe: &Ffprobe) -> anyhow::Result<Option<i32>> {
        const KEYINT_DEFAULT_INPUT_MIN: Duration = Duration::from_secs(60 * 3);
        const KEYINT_DEFAULT: Duration = Duration::from_secs(10);

        let filter_fps = self.vfilter.as_deref().and_then(try_parse_fps_vfilter);
        Ok(
            match (self.keyint, &probe.duration, &probe.fps, filter_fps) {
                // use the filter-fps if used, otherwise the input fps
                (Some(ki), .., Some(fps)) => Some(ki.keyint_number(Ok(fps))?),
                (Some(ki), _, fps, None) => Some(ki.keyint_number(fps.clone())?),
                (None, Ok(duration), _, Some(fps)) if *duration >= KEYINT_DEFAULT_INPUT_MIN => {
                    Some(KeyInterval::Duration(KEYINT_DEFAULT).keyint_number(Ok(fps))?)
                }
                (None, Ok(duration), Ok(fps), None) if *duration >= KEYINT_DEFAULT_INPUT_MIN => {
                    Some(KeyInterval::Duration(KEYINT_DEFAULT).keyint_number(Ok(*fps))?)
                }
                _ => None,
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Encoder {
    SvtAv1,
    Ffmpeg(Arc<str>),
}

impl Encoder {
    /// vcodec name that would work if you used it as the -e argument.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SvtAv1 => "svt-av1",
            Self::Ffmpeg(vcodec) => vcodec,
        }
    }
}

impl std::str::FromStr for Encoder {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "svt-av1" => Self::SvtAv1,
            vcodec => Self::Ffmpeg(vcodec.into()),
        })
    }
}

#[derive(Debug, Clone)]
pub enum EncoderArgs<'a> {
    SvtAv1(SvtArgs<'a>),
    Ffmpeg(FfmpegEncodeArgs<'a>),
}

impl EncoderArgs<'_> {
    pub fn pixel_format(&self) -> PixelFormat {
        match self {
            Self::SvtAv1(a) => a.pix_fmt,
            Self::Ffmpeg(a) => a.pix_fmt,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

    pub fn input_depth(self) -> &'static str {
        match self {
            Self::Yuv420p10le => "10",
            Self::Yuv444p10le => "10",
            Self::Yuv420p => "8",
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

/// Should use keyint & scd defaults for >3m inputs.
#[test]
fn to_svt_args_default_over_3m() {
    let svt = Encode {
        encoder: Encoder::SvtAv1,
        input: "vid.mp4".into(),
        vfilter: Some("scale=320:-1,fps=film".into()),
        preset: None,
        pix_format: None,
        keyint: None,
        scd: None,
        svt_args: vec!["film-grain=30".into()],
        enc_args: <_>::default(),
        enc_input_args: <_>::default(),
    };

    let probe = Ffprobe {
        duration: Ok(Duration::from_secs(300)),
        has_audio: true,
        max_audio_channels: None,
        fps: Ok(30.0),
        resolution: Some((1280, 720)),
        has_image_extension: false,
        pix_fmt: None,
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
    } = svt.to_svt_args(32.0, &probe).expect("to_svt_args");

    assert_eq!(input, svt.input);
    assert_eq!(vfilter, Some("scale=320:-1,fps=film"));
    assert_eq!(crf, 32.0);
    assert_eq!(preset, 8);
    assert_eq!(pix_fmt, PixelFormat::Yuv420p10le);
    assert_eq!(keyint, Some(240)); // based off filter fps
    assert_eq!(scd, 1);
    assert_eq!(args, vec!["film-grain", "30"]);
}

#[test]
fn to_svt_args_default_under_3m() {
    let svt = Encode {
        encoder: Encoder::SvtAv1,
        input: "vid.mp4".into(),
        vfilter: None,
        preset: Some(Preset::Number(7)),
        pix_format: Some(PixelFormat::Yuv420p),
        keyint: None,
        scd: None,
        svt_args: vec![],
        enc_args: <_>::default(),
        enc_input_args: <_>::default(),
    };

    let probe = Ffprobe {
        duration: Ok(Duration::from_secs(179)),
        has_audio: true,
        max_audio_channels: None,
        fps: Ok(24.0),
        resolution: Some((1280, 720)),
        has_image_extension: false,
        pix_fmt: None,
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
    } = svt.to_svt_args(32.0, &probe).expect("to_svt_args");

    assert_eq!(input, svt.input);
    assert_eq!(vfilter, None);
    assert_eq!(crf, 32.0);
    assert_eq!(preset, 7);
    assert_eq!(pix_fmt, PixelFormat::Yuv420p);
    assert_eq!(keyint, None);
    assert_eq!(scd, 0);
    assert!(args.is_empty());
}
