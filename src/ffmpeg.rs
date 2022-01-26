//! ffmpeg logic
use crate::SAMPLE_SIZE_S;
use anyhow::{anyhow, Context};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Create a 20s sample from `sample_start`, or re-use if it already exists.
pub fn cut_sample(
    input: &Path,
    sample_start: Duration,
) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Error>)> {
    let ext = input
        .extension()
        .context("input has no extension")?
        .to_string_lossy();
    let dest = input.with_extension(format!(
        "sample{}+{SAMPLE_SIZE_S}.{ext}",
        sample_start.as_secs()
    ));

    let output: ProcessChunkStream = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-ss")
        .arg(sample_start.as_secs().to_string())
        .arg("-t")
        .arg(SAMPLE_SIZE_S.to_string())
        .arg("-c:v")
        .arg("copy")
        .arg("-an")
        .arg(&dest)
        .try_into()
        .context("ffmpeg cut")?;

    let output = output.filter_map(|item| match item {
        Item::Done(code) => match code {
            Ok(c) if c.success() => None,
            Ok(c) => Some(anyhow!("ffmpeg cut exit code {:?}", c.code())),
            Err(err) => Some(err.into()),
        },
        _ => None,
    });

    Ok((dest, output))
}

#[derive(Debug, PartialEq)]
pub struct FfmpegProgress {
    pub frame: u64,
    pub fps: f32,
}

impl FfmpegProgress {
    pub fn try_parse(out: &str) -> Option<Self> {
        if out.starts_with("frame=") && out.ends_with('\r') {
            let frame: u64 = parse_label_substr("frame=", out)?.parse().ok()?;
            let fps: f32 = parse_label_substr("fps=", out)?.parse().ok()?;
            return Some(Self { frame, fps });
        }
        None
    }
}

/// Parse a ffmpeg `label=  value ` type substring.
fn parse_label_substr(label: &str, s: &str) -> Option<String> {
    let idx = s.find(label)?;
    Some(
        s[idx + label.len()..]
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| !c.is_whitespace())
            .collect(),
    )
}

#[test]
fn parse_ffmpeg_out() {
    let out = "frame=  288 fps= 94 q=-0.0 size=N/A time=00:00:12.00 bitrate=N/A speed=3.94x    \r";
    assert_eq!(
        FfmpegProgress::try_parse(out),
        Some(FfmpegProgress {
            frame: 288,
            fps: 94.0,
        })
    );
}
