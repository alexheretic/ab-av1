//! svt-av1 logic
use crate::{
    process::{exit_ok_option, CommandExt, FfmpegProgress},
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

    let (yuv_out, yuv_pipe) = yuv::pipe420p10le(sample)?;

    let svt = Command::new("SvtAv1EncApp")
        .kill_on_drop(true)
        .arg2("-i", "stdin")
        .arg2("--crf", crf.to_string())
        .arg2("--preset", preset.to_string())
        .arg2("--input-depth", "10")
        .arg2("-b", &dest)
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
    has_audio: bool,
    audio_codec: Option<&str>,
    audio_quality: Option<&str>,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<FfmpegProgress>>> {
    let output_mp4 = output.extension().and_then(|e| e.to_str()) == Some("mp4");

    let audio_codec = audio_codec.unwrap_or_else(|| match input.extension() {
        // use `-c:a copy` if the extensions are the same, otherwise reencode with opus
        ext if ext.is_some() && ext == output.extension() => "copy",
        _ => "libopus",
    });

    let (yuv_out, yuv_pipe) = yuv::pipe420p10le(input)?;
    let yuv_pipe = yuv_pipe.filter(Result::is_err);

    let mut svt = Command::new("SvtAv1EncApp")
        .kill_on_drop(true)
        .arg2("-i", "stdin")
        .arg2("--crf", crf.to_string())
        .arg2("--preset", preset.to_string())
        .arg2("--input-depth", "10")
        .arg2("-b", "stdout")
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

    let to_output = Command::new("ffmpeg")
        .kill_on_drop(true)
        .stdin(svt_out)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .arg("-y")
        .arg2("-i", "-")
        .arg2_if(has_audio, "-i", input)
        .arg2_if(has_audio, "-map", "0:v")
        .arg2_if(has_audio, "-map", "1:a:0")
        .arg2_if(has_audio, "-c:a", audio_codec)
        .arg2_opt("-aq", audio_quality)
        .arg2("-c:v", "copy")
        .arg2_if(output_mp4, "-movflags", "+faststart")
        .arg(output)
        .spawn()
        .context("ffmpeg to-output")?;

    let to_mp4 = FfmpegProgress::stream(to_output, "ffmpeg to-output");

    Ok(yuv_pipe.merge(svt).merge(to_mp4))
}
