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
    vfilter: Option<&str>,
) -> anyhow::Result<(Stdio, impl Stream<Item = anyhow::Result<FfmpegProgress>>)> {
    let mut yuv4mpegpipe = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg2("-i", input)
        .arg2("-pix_fmt", pix_fmt.as_str())
        .arg2_opt("-vf", vfilter)
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

#[cfg(unix)]
pub mod unix {
    use super::*;
    use rand::{
        distributions::{Alphanumeric, DistString},
        thread_rng,
    };
    use std::path::PathBuf;

    /// ffmpeg yuv4mpegpipe returning the temporary fifo path & [`FfmpegProgress`] stream.
    pub fn pipe_to_fifo(
        input: &Path,
        pix_fmt: PixelFormat,
    ) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegProgress>>)> {
        let fifo = PathBuf::from(format!(
            "/tmp/ab-av1-{}.fifo",
            Alphanumeric.sample_string(&mut thread_rng(), 12)
        ));
        unix_named_pipe::create(&fifo, None)?;
        crate::temporary::add(&fifo);

        let yuv4mpegpipe = Command::new("ffmpeg")
            .kill_on_drop(true)
            .arg2("-i", input)
            .arg2("-pix_fmt", pix_fmt.as_str())
            .arg2("-strict", "-1")
            .arg2("-f", "yuv4mpegpipe")
            .arg("-y")
            .arg(&fifo)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("ffmpeg yuv4mpegpipe")?;
        let stream = FfmpegProgress::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");
        Ok((fifo, stream))
    }
}
