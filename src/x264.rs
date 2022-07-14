//! x264 logic
use crate::{
    command::args::PixelFormat,
    process::{CommandExt, FfmpegOut},
    svtav1,
    temporary::{self, TempKind},
};
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;
use tokio_stream::Stream;

/// Exposed ffmpeg libx264 / libx265 encoding args.
#[derive(Debug, Clone)]
pub struct X26xArgs<'a> {
    pub input: &'a Path,
    pub vfilter: Option<&'a str>,
    pub pix_fmt: PixelFormat,
    pub crf: u8,
    pub preset: Option<&'a str>,
    pub keyint: Option<i32>,
}

/// Encode a sample.
pub fn encode_sample(
    X26xArgs {
        input,
        vfilter,
        pix_fmt,
        crf,
        preset,
        keyint,
    }: X26xArgs,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
    let dest_ext = input
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("mp4");
    let mut dest = match preset {
        Some(p) => input.with_extension(format!("x264.crf{crf}.{p}.{dest_ext}")),
        None => input.with_extension(format!("x264.crf{crf}.{dest_ext}")),
    };
    if let (Some(mut temp), Some(name)) = (temp_dir, dest.file_name()) {
        temp.push(name);
        dest = temp;
    }
    temporary::add(&dest, TempKind::Keepable);

    let enc = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg2("-i", input)
        .arg2("-c:v", "libx264")
        .arg2("-crf", crf)
        .arg2_opt("-x264-params", keyint.map(|i| format!("keyint={i}")))
        .arg2("-pix_fmt", pix_fmt.as_str())
        .arg2_opt("-preset", preset)
        .arg2_opt("-vf", vfilter)
        .arg("-an")
        .arg(&dest)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg libx264")?;

    let stream = FfmpegOut::stream(enc, "ffmpeg libx264");
    Ok((dest, stream))
}

/// Encode to mp4 including re-encoding audio with libopus, if present.
pub fn encode(
    X26xArgs {
        input,
        vfilter,
        pix_fmt,
        crf,
        preset,
        keyint,
    }: X26xArgs,
    output: &Path,
    has_audio: bool,
    audio_codec: Option<&str>,
    downmix_to_stereo: bool,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<FfmpegOut>>> {
    let output_is_mp4 = output.extension().and_then(|e| e.to_str()) == Some("mp4");

    let audio_codec = audio_codec.unwrap_or_else(|| {
        svtav1::default_audio_codec(input, output, downmix_to_stereo, has_audio)
    });

    let enc = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg2("-i", input)
        .arg2("-c:v", "libx264")
        .arg2("-crf", crf)
        .arg2_opt("-x264-params", keyint.map(|i| format!("keyint={i}")))
        .arg2("-pix_fmt", pix_fmt.as_str())
        .arg2_opt("-preset", preset)
        .arg2_opt("-vf", vfilter)
        .arg2("-c:s", "copy")
        .arg2("-c:a", audio_codec)
        .arg2_if(downmix_to_stereo, "-ac", 2)
        .arg2_if(audio_codec == "libopus", "-b:a", "128k")
        .arg2_if(output_is_mp4, "-movflags", "+faststart")
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg libx264")?;

    Ok(FfmpegOut::stream(enc, "ffmpeg libx264"))
}
