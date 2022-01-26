//! svt-av1 logic
use crate::ffmpeg::FfmpegProgress;
use anyhow::{anyhow, Context};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

pub fn encode_ivf(
    sample: &Path,
    crf: u8,
    preset: u8,
) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegProgress>>)> {
    let dest = sample.with_extension(format!("crf{crf}.p{preset}.ivf"));

    let mut yuv4mpegpipe = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg("-i")
        .arg(sample)
        .arg("-strict")
        .arg("-1")
        .arg("-f")
        .arg("yuv4mpegpipe")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg yuv4mpegpipe")?;

    let yuv4mpegpipe_out: Stdio = yuv4mpegpipe.stdout.take().unwrap().try_into().unwrap();
    let yuv4mpegpipe = ProcessChunkStream::from(yuv4mpegpipe).filter_map(|item| match item {
        Item::Stderr(chunk) => FfmpegProgress::try_parse(&String::from_utf8_lossy(&chunk)).map(Ok),
        Item::Stdout(_) => None,
        Item::Done(code) => match code {
            Ok(c) if c.success() => None,
            Ok(c) => Some(Err(anyhow!("ffmpeg yuv4mpegpipe exit code {:?}", c.code()))),
            Err(err) => Some(Err(err.into())),
        },
    });

    let svt = Command::new("SvtAv1EncApp")
        .arg("-i")
        .arg("stdin")
        .arg("--crf")
        .arg(crf.to_string())
        .arg("--preset")
        .arg(preset.to_string())
        .arg("-b")
        .arg(&dest)
        .stdin(yuv4mpegpipe_out)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("SvtAv1EncApp")?;
    let svt = ProcessChunkStream::from(svt).filter_map(|item| match item {
        Item::Done(code) => match code {
            Ok(c) if c.success() => None,
            Ok(c) => Some(Err(anyhow!("SvtAv1EncApp exit code {:?}", c.code()))),
            Err(err) => Some(Err(err.into())),
        },
        _ => None,
    });

    Ok((dest, yuv4mpegpipe.merge(svt)))
}
