//! svt-av1 logic
use crate::{
    process::{exit_ok_option, FfmpegProgress},
    temporary, yuv,
};
use anyhow::Context;
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
    temporary::add(&dest);

    let (yuv_out, yuv_pipe) = yuv::yuv4mpegpipe(sample)?;

    let svt = Command::new("SvtAv1EncApp")
        .kill_on_drop(true)
        .arg("-i")
        .arg("stdin")
        .arg("--crf")
        .arg(crf.to_string())
        .arg("--preset")
        .arg(preset.to_string())
        .arg("-b")
        .arg(&dest)
        .stdin(yuv_out)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("SvtAv1EncApp")?;
    let svt = ProcessChunkStream::from(svt).filter_map(|item| match item {
        Item::Done(code) => exit_ok_option("SvtAv1EncApp", code),
        _ => None,
    });

    Ok((dest, yuv_pipe.merge(svt)))
}

/// Encode to mp4 including re-encoding audio with libopus, if present.
pub fn encode(
    input: &Path,
    crf: u8,
    preset: u8,
    output: &Path,
    audio: bool,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<FfmpegProgress>>> {
    anyhow::ensure!(
        output.extension().and_then(|e| e.to_str()) == Some("mp4"),
        "Only mp4 output is supported"
    );

    let (yuv_out, yuv_pipe) = yuv::yuv4mpegpipe(input)?;
    let yuv_pipe = yuv_pipe.filter_map(|p| match p {
        Ok(_) => None,
        Err(_) => Some(p),
    });

    let mut svt = Command::new("SvtAv1EncApp")
        .kill_on_drop(true)
        .arg("-i")
        .arg("stdin")
        .arg("--crf")
        .arg(crf.to_string())
        .arg("--preset")
        .arg(preset.to_string())
        .arg("-b")
        .arg("stdout")
        .stdin(yuv_out)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("SvtAv1EncApp")?;
    let svt_out: Stdio = svt.stdout.take().unwrap().try_into().unwrap();
    let svt = ProcessChunkStream::from(svt).filter_map(|item| match item {
        Item::Done(code) => exit_ok_option("SvtAv1EncApp", code),
        _ => None,
    });

    let to_mp4 = match audio {
        false => Command::new("ffmpeg")
            .kill_on_drop(true)
            .stdin(svt_out)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .arg("-y")
            .arg("-i")
            .arg("-")
            .arg("-c:v")
            .arg("copy")
            .arg("-movflags")
            .arg("+faststart")
            .arg(output)
            .spawn(),
        true => Command::new("ffmpeg")
            .kill_on_drop(true)
            .stdin(svt_out)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
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
            .spawn(),
    }
    .context("ffmpeg to-mp4")?;

    let to_mp4 = FfmpegProgress::stream(to_mp4, "ffmpeg to-mp4");

    Ok(yuv_pipe.merge(svt).merge(to_mp4))
}
