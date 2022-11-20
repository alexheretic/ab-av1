use crate::{
    command::args::PixelFormat,
    process::{CommandExt, FfmpegOut},
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
) -> anyhow::Result<(Stdio, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
    let mut yuv4mpegpipe = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg2("-i", input)
        .arg2("-pix_fmt", pix_fmt.as_str())
        .arg2_opt("-vf", vfilter)
        .arg2("-strict", "-1")
        .arg2("-f", "yuv4mpegpipe")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg yuv4mpegpipe")?;
    let stdout = yuv4mpegpipe.stdout.take().unwrap().try_into().unwrap();
    let stream = FfmpegOut::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");
    Ok((stdout, stream))
}

#[cfg(windows)]
pub mod windows {
    use super::*;

    pub fn named_pipe(
        input: &Path,
        pix_fmt: PixelFormat,
    ) -> anyhow::Result<(String, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
        use rand::distributions::{Alphanumeric, DistString};
        use rand::thread_rng;

        let mut in_name = Alphanumeric.sample_string(&mut thread_rng(), 12);
        in_name.insert_str(0, r"\\.\pipe\ab-av1-in-");

        let mut in_server = tokio::net::windows::named_pipe::ServerOptions::new()
            .access_outbound(false)
            .first_pipe_instance(true)
            .max_instances(1)
            .create(&in_name)?;

        let out_name = in_name.replacen("-in-", "-out-", 1);
        let mut out_server = tokio::net::windows::named_pipe::ServerOptions::new()
            .access_inbound(false)
            .first_pipe_instance(true)
            .max_instances(1)
            .create(&out_name)?;

        tokio::spawn(async move {
            in_server.connect().await.unwrap();
            in_server.readable().await.unwrap();
            out_server.connect().await.unwrap();
            out_server.writable().await.unwrap();
            tokio::io::copy(&mut in_server, &mut out_server)
                .await
                .unwrap();
        });

        let yuv4mpegpipe = Command::new("ffmpeg")
            .kill_on_drop(true)
            .arg2("-i", input)
            .arg2("-pix_fmt", pix_fmt.as_str())
            .arg2("-strict", "-1")
            .arg2("-f", "yuv4mpegpipe")
            .arg("-y")
            .arg(&in_name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("ffmpeg yuv4mpegpipe")?;
        let stream = FfmpegOut::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");

        Ok((out_name, stream))
    }
}

#[cfg(unix)]
pub mod unix {
    use super::*;
    use crate::temporary::{self, TempKind};
    use rand::{
        distributions::{Alphanumeric, DistString},
        thread_rng,
    };
    use std::path::PathBuf;

    /// ffmpeg yuv4mpegpipe returning the temporary fifo path & [`FfmpegProgress`] stream.
    pub fn pipe_to_fifo(
        input: &Path,
        pix_fmt: PixelFormat,
    ) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
        let fifo = PathBuf::from(format!(
            "/tmp/ab-av1-{}.fifo",
            Alphanumeric.sample_string(&mut thread_rng(), 12)
        ));
        unix_named_pipe::create(&fifo, None)?;
        temporary::add(&fifo, TempKind::NotKeepable);

        let yuv4mpegpipe = Command::new("ffmpeg")
            .kill_on_drop(true)
            .arg2("-i", input)
            .arg2("-pix_fmt", pix_fmt.as_str())
            .arg2("-strict", "-1")
            .arg2("-f", "yuv4mpegpipe")
            .arg("-y")
            .arg(&fifo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("ffmpeg yuv4mpegpipe")?;
        let stream = FfmpegOut::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");
        Ok((fifo, stream))
    }
}
