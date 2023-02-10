//! ffprobe logic
use crate::command::args::PixelFormat;
use anyhow::{anyhow, Context};
use std::{fmt, fs::File, io::Read, path::Path, time::Duration};

pub struct Ffprobe {
    /// Duration of video.
    pub duration: Result<Duration, ProbeError>,
    /// The video has audio stream(s).
    pub has_audio: bool,
    /// Audio number of channels (if multiple channel the highest).
    pub max_audio_channels: Option<i64>,
    /// Video frame rate.
    pub fps: Result<f64, ProbeError>,
    pub resolution: Option<(u32, u32)>,
    pub is_image: bool,
    pub pix_fmt: Option<String>,
}

impl Ffprobe {
    pub fn pixel_format(&self) -> Option<PixelFormat> {
        let pf = self.pix_fmt.as_deref()?;
        PixelFormat::try_from(pf).ok()
    }

    pub fn nframes(&self) -> Result<u64, ProbeError> {
        match (&self.fps, &self.duration) {
            (Ok(fps), Ok(duration)) => {
                let frames = (fps * duration.as_secs_f64()).round();
                if frames.is_normal() && frames.is_sign_positive() {
                    Ok(frames as _)
                } else {
                    Err(ProbeError(format!("Invalid nframes {frames}")))
                }
            }
            (Err(e), _) | (_, Err(e)) => Err(e.clone()),
        }
    }
}

/// Try to ffprobe the given input.
pub fn probe(input: &Path) -> Ffprobe {
    let is_image = is_image(input).unwrap_or(false);

    let probe = match ffprobe::ffprobe(input) {
        Ok(p) => p,
        Err(err) => {
            return Ffprobe {
                duration: Err(ProbeError(format!("ffprobe: {err}"))),
                fps: Err(ProbeError(format!("ffprobe: {err}"))),
                has_audio: true,
                max_audio_channels: None,
                resolution: None,
                is_image: false,
                pix_fmt: None,
            }
        }
    };

    let fps = read_fps(&probe);
    let duration = read_duration(&probe);
    let has_audio = probe
        .streams
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio"));
    let max_audio_channels = probe
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .filter_map(|a| a.channels)
        .max();

    let resolution = probe
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("video"))
        .find_map(|s| {
            let w = s.width.and_then(|w| u32::try_from(w).ok())?;
            let h = s.height.and_then(|w| u32::try_from(w).ok())?;
            Some((w, h))
        });

    let pix_fmt = probe
        .streams
        .into_iter()
        .filter(|s| s.codec_type.as_deref() == Some("video"))
        .find_map(|s| s.pix_fmt);

    Ffprobe {
        duration: duration.map_err(ProbeError::from),
        fps: fps.map_err(ProbeError::from),
        has_audio,
        max_audio_channels,
        resolution,
        is_image,
        pix_fmt,
    }
}

fn is_image(path: &Path) -> anyhow::Result<bool> {
    let file = File::open(path)?;
    let mut file_header = Vec::with_capacity(8192);
    file.take(8192).read_to_end(&mut file_header)?;

    Ok(infer::is_image(&file_header))
}

fn read_duration(probe: &ffprobe::FfProbe) -> anyhow::Result<Duration> {
    match probe.format.duration.as_deref() {
        Some(duration_s) => {
            let duration_f = duration_s
                .parse::<f64>()
                .with_context(|| format!("invalid ffprobe video duration: {duration_s:?}"))?;
            Duration::try_from_secs_f64(duration_f)
                .map_err(|e| anyhow!("{e}: ffprobe video duration: {duration_s:?}"))
        }
        None => Ok(Duration::ZERO),
    }
}

fn read_fps(probe: &ffprobe::FfProbe) -> anyhow::Result<f64> {
    let vstream = probe
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .context("no video stream found")?;

    parse_frame_rate(&vstream.avg_frame_rate)
        .or_else(|| parse_frame_rate(&vstream.r_frame_rate))
        .context("invalid ffprobe video frame rate")
}

/// parse "x/y" or float strings.
pub fn parse_frame_rate(rate: &str) -> Option<f64> {
    if let Some((x, y)) = rate.split_once('/') {
        let x: f64 = x.parse().ok()?;
        let y: f64 = y.parse().ok()?;
        if x <= 0.0 || y <= 0.0 {
            return None;
        }
        Some(x / y)
    } else {
        rate.parse()
            .ok()
            .filter(|f: &f64| f.is_finite() && *f > 0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError(String);

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<anyhow::Error> for ProbeError {
    fn from(err: anyhow::Error) -> Self {
        Self(format!("{err}"))
    }
}

impl std::error::Error for ProbeError {}
