//! ffprobe logic
use anyhow::{anyhow, Context};
use std::{path::Path, time::Duration};

pub struct Ffprobe {
    /// Duration of video.
    pub duration: anyhow::Result<Duration>,
    /// The video has audio stream.
    pub has_audio: bool,
    /// Video frame rate.
    pub fps: anyhow::Result<f64>,
}

/// Try to ffprobe the given input.
pub fn probe(input: &Path) -> Ffprobe {
    let probe = match ffprobe::ffprobe(&input) {
        Ok(p) => p,
        Err(err) => {
            return Ffprobe {
                duration: Err(anyhow!("ffprobe: {err}")),
                fps: Err(anyhow!("ffprobe: {err}")),
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
        duration,
        fps,
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

/// parse "x/y" strings.
fn parse_frame_rate(rate: &str) -> Option<f64> {
    let (x, y) = rate.split_once('/')?;
    let x: f64 = x.parse().ok()?;
    let y: f64 = y.parse().ok()?;
    if x <= 0.0 || y <= 0.0 {
        return None;
    }
    Some(x / y)
}
