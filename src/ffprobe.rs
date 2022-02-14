//! ffprobe logic
use anyhow::Context;
use std::{fmt, path::Path, time::Duration};

pub struct Ffprobe {
    /// Duration of video.
    pub duration: Result<Duration, ProbeError>,
    /// The video has audio stream(s).
    pub has_audio: bool,
    /// Video frame rate.
    pub fps: Result<f64, ProbeError>,
}

/// Try to ffprobe the given input.
pub fn probe(input: &Path) -> Ffprobe {
    let probe = match ffprobe::ffprobe(&input) {
        Ok(p) => p,
        Err(err) => {
            return Ffprobe {
                duration: Err(ProbeError(format!("ffprobe: {err}"))),
                fps: Err(ProbeError(format!("ffprobe: {err}"))),
                has_audio: true,
            }
        }
    };

    let fps = read_fps(&probe);
    let duration = read_duration(&probe);
    let has_audio = probe
        .streams
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio"));

    Ffprobe {
        duration: duration.map_err(ProbeError::from),
        fps: fps.map_err(ProbeError::from),
        has_audio,
    }
}

fn read_duration(probe: &ffprobe::FfProbe) -> anyhow::Result<Duration> {
    let duration_s = probe
        .format
        .duration
        .as_deref()
        .context("ffprobe missing video duration")?
        .parse::<f64>()
        .context("invalid ffprobe video duration")?;
    Ok(Duration::from_secs_f64(duration_s))
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

#[derive(Debug, Clone, PartialEq)]
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
