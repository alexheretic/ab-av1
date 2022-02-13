use crate::{
    command::args::PixelFormat,
    process::{CommandExt, FfmpegProgress},
};
use anyhow::Context;
use std::{path::Path, process::Stdio};
use tokio::process::Command;
use tokio_stream::Stream;

/// ffmpeg yuv4mpegpipe returning the stdout & [`FfmpegProgress`] stream.
pub fn pipe(
    input: &Path,
    pix_fmt: PixelFormat,
) -> anyhow::Result<(Stdio, impl Stream<Item = anyhow::Result<FfmpegProgress>>)> {
    let mut yuv4mpegpipe = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg2("-i", input)
        .arg2("-pix_fmt", pix_fmt.as_str())
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
