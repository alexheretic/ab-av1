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

/// Encode to ivf. Used for sample encoding.
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

/// Encode to mp4 including re-encoding audio with libopus.
pub fn encode(
    input: &Path,
    crf: u8,
    preset: u8,
    output: &Path,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<FfmpegProgress>>> {
    anyhow::ensure!(
        output.extension().and_then(|e| e.to_str()) == Some("mp4"),
        "Only mp4 supported"
    );

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
        .stderr(Stdio::null())
        .spawn()
        .context("ffmpeg yuv4mpegpipe")?;
    let yuv4mpegpipe_out: Stdio = yuv4mpegpipe.stdout.take().unwrap().try_into().unwrap();
    let yuv4mpegpipe = ProcessChunkStream::from(yuv4mpegpipe).filter_map(|item| match item {
        Item::Done(code) => match code {
            Ok(c) if c.success() => None,
            Ok(c) => Some(Err(anyhow!("ffmpeg yuv4mpegpipe exit code {:?}", c.code()))),
            Err(err) => Some(Err(err.into())),
        },
        _ => None,
    });

    let mut svt = Command::new("SvtAv1EncApp")
        .arg("-i")
        .arg("stdin")
        .arg("--crf")
        .arg(crf.to_string())
        .arg("--preset")
        .arg(preset.to_string())
        .arg("-b")
        .arg("stdout")
        .stdin(yuv4mpegpipe_out)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("SvtAv1EncApp")?;
    let svt_out: Stdio = svt.stdout.take().unwrap().try_into().unwrap();
    let svt = ProcessChunkStream::from(svt).filter_map(|item| match item {
        Item::Done(code) => match code {
            Ok(c) if c.success() => None,
            Ok(c) => Some(Err(anyhow!("SvtAv1EncApp exit code {:?}", c.code()))),
            Err(err) => Some(Err(err.into())),
        },
        _ => None,
    });

    let to_mp4 = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg("-")
        .arg("-i")
        .arg(input)
        .arg("-map")
        .arg("0:v")
        .arg("-map")
        .arg("1:a:0")
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("libopus")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output)
        .stdin(svt_out)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg to-mp4")?;

    let to_mp4 = ProcessChunkStream::from(to_mp4).filter_map(|item| match item {
        Item::Stderr(chunk) => FfmpegProgress::try_parse(&String::from_utf8_lossy(&chunk)).map(Ok),
        Item::Stdout(_) => None,
        Item::Done(code) => match code {
            Ok(c) if c.success() => None,
            Ok(c) => Some(Err(anyhow!("ffmpeg to-mp4 exit code {:?}", c.code()))),
            Err(err) => Some(Err(err.into())),
        },
    });

    Ok(yuv4mpegpipe.merge(svt).merge(to_mp4))
}
