use crate::process::FfmpegProgress;
use anyhow::Context;
use std::{path::Path, process::Stdio};
use tokio::process::Command;
use tokio_stream::Stream;

/// ffmpeg yuv4mpegpipe returning the stdout & [`FfmpegProgress`] stream.
pub fn yuv4mpegpipe(
    input: &Path,
) -> anyhow::Result<(Stdio, impl Stream<Item = anyhow::Result<FfmpegProgress>>)> {
    let mut yuv4mpegpipe = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg("-i")
        .arg(input)
        .arg("-strict")
        .arg("-1")
        .arg("-f")
        .arg("yuv4mpegpipe")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg yuv4mpegpipe")?;
    let stdout = yuv4mpegpipe.stdout.take().unwrap().try_into().unwrap();
    let stream = FfmpegProgress::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");
    Ok((stdout, stream))
}
