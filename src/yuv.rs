use crate::process::{CommandExt, FfmpegProgress};
use anyhow::Context;
use std::{path::Path, process::Stdio};
use tokio::process::Command;
use tokio_stream::Stream;

/// ffmpeg yuv420p10le yuv4mpegpipe returning the stdout & [`FfmpegProgress`] stream.
pub fn pipe420p10le(
    input: &Path,
) -> anyhow::Result<(Stdio, impl Stream<Item = anyhow::Result<FfmpegProgress>>)> {
    let mut yuv4mpegpipe = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg2("-i", input)
        .arg2("-pix_fmt", "yuv420p10le")
        .arg2("-strict", "-1")
        .arg2("-f", "yuv4mpegpipe")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg yuv4mpegpipe")?;
    let stdout = yuv4mpegpipe.stdout.take().unwrap().try_into().unwrap();
    let stream = FfmpegProgress::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");
    Ok((stdout, stream))
}
