//! ffmpeg logic
use crate::{process::ensure_success, temporary, SAMPLE_SIZE_S};
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use time::macros::format_description;
use tokio::process::Command;

/// Create a 20s sample from `sample_start`.
///
/// Fast as this uses `-c:v copy`.
pub async fn copy(input: &Path, sample_start: Duration) -> anyhow::Result<PathBuf> {
    let ext = input
        .extension()
        .context("input has no extension")?
        .to_string_lossy();
    let dest = input.with_extension(format!(
        "sample{}+{SAMPLE_SIZE_S}.{ext}",
        sample_start.as_secs()
    ));

    temporary::add(&dest);

    let out = Command::new("ffmpeg")
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
        .output()
        .await
        .context("ffmpeg copy")?;

    ensure_success("ffmpeg copy", &out)?;
    Ok(dest)
}

#[derive(Debug, PartialEq)]
pub struct FfmpegProgress {
    pub frame: u64,
    pub fps: f32,
    pub time: Duration,
}

impl FfmpegProgress {
    pub fn try_parse(out: &str) -> Option<Self> {
        if out.starts_with("frame=") && out.ends_with('\r') {
            let frame: u64 = parse_label_substr("frame=", out)?.parse().ok()?;
            let fps: f32 = parse_label_substr("fps=", out)?.parse().ok()?;
            let (h, m, s, ns) = time::Time::parse(
                parse_label_substr("time=", out)?,
                &format_description!("[hour]:[minute]:[second].[subsecond]"),
            )
            .unwrap()
            .as_hms_nano();
            return Some(Self {
                frame,
                fps,
                time: Duration::new(h as u64 * 60 * 60 + m as u64 * 60 + s as u64, ns),
            });
        }
        None
    }
}

/// Parse a ffmpeg `label=  value ` type substring.
fn parse_label_substr<'a>(label: &str, line: &'a str) -> Option<&'a str> {
    let line = &line[line.find(label)? + label.len()..];
    let val_start = line.char_indices().find(|(_, c)| !c.is_whitespace())?.0;
    let val_end = val_start
        + line[val_start..]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| line[val_start..].len());

    Some(&line[val_start..val_end])
}

#[test]
fn parse_ffmpeg_out() {
    let out = "frame=  288 fps= 94 q=-0.0 size=N/A time=01:23:12.34 bitrate=N/A speed=3.94x    \r";
    assert_eq!(
        FfmpegProgress::try_parse(out),
        Some(FfmpegProgress {
            frame: 288,
            fps: 94.0,
            time: Duration::new(60 * 60 + 23 * 60 + 12, 340_000_000),
        })
    );
}
