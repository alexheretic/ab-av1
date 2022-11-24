//! svt-av1 logic
use crate::{
    command::args::PixelFormat,
    process::{exit_ok_option, CommandExt, FfmpegOut},
    temporary::{self, TempKind},
    yuv,
};
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Exposed SvtAv1EncApp args.
///
/// See <https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/svt-av1_encoder_user_guide.md>
#[derive(Debug, Clone)]
pub struct SvtArgs<'a> {
    pub input: &'a Path,
    pub vfilter: Option<&'a str>,
    pub pix_fmt: PixelFormat,
    pub crf: u8,
    pub preset: u8,
    pub keyint: Option<i32>,
    pub scd: u8,
    pub args: Vec<&'a str>,
}

/// Encode to sample ivf.
pub fn encode_sample(
    SvtArgs {
        input,
        vfilter,
        pix_fmt,
        crf,
        preset,
        keyint,
        scd,
        args,
    }: SvtArgs,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
    let mut dest = input.with_extension(format!("crf{crf}.p{preset}.ivf"));
    if let (Some(mut temp), Some(name)) = (temp_dir, dest.file_name()) {
        temp.push(name);
        dest = temp;
    }
    temporary::add(&dest, TempKind::Keepable);

    let (yuv_out, yuv_pipe) = yuv::pipe(input, pix_fmt, vfilter)?;

    let svt = Command::new("SvtAv1EncApp")
        .kill_on_drop(true)
        .arg2("-i", "stdin")
        .arg2("--crf", crf)
        .arg2("--preset", preset)
        .arg2("--input-depth", pix_fmt.input_depth())
        .arg2_opt("--keyint", keyint)
        .arg2("--scd", scd)
        .arg2("-b", &dest)
        .args(args)
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

/// Encode to output (e.g. mkv).
pub fn encode(
    SvtArgs {
        input,
        vfilter,
        pix_fmt,
        crf,
        preset,
        keyint,
        scd,
        args,
    }: SvtArgs,
    output: &Path,
    has_audio: bool,
    audio_codec: Option<&str>,
    downmix_to_stereo: bool,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<FfmpegOut>>> {
    let output_ext = output.extension().and_then(|e| e.to_str());
    let add_faststart = output_ext == Some("mp4");
    let add_cues_to_front = output_ext == Some("mkv");

    let audio_codec = audio_codec
        .unwrap_or_else(|| default_audio_codec(input, output, downmix_to_stereo, has_audio));

    let (yuv_out, yuv_pipe) = yuv::pipe(input, pix_fmt, vfilter)?;
    let yuv_pipe = yuv_pipe.filter(Result::is_err);

    let mut svt = Command::new("SvtAv1EncApp")
        .kill_on_drop(true)
        .arg2("-i", "stdin")
        .arg2("--crf", crf)
        .arg2("--preset", preset)
        .arg2("--input-depth", pix_fmt.input_depth())
        .arg2_opt("--keyint", keyint)
        .arg2("--scd", scd)
        .arg2("-b", "stdout")
        .args(args)
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
        .arg("-y")
        .arg2("-i", "-")
        .arg2("-i", input)
        // use av1 video
        .arg2("-map", "0:v")
        // map everything but video from input
        .arg2("-map", "1")
        .arg2("-map", "-1:v")
        .arg2("-c:s", "copy")
        .arg2("-c:a", audio_codec)
        .arg2_if(downmix_to_stereo, "-ac", 2)
        .arg2("-c:v", "copy")
        .arg2_if(audio_codec == "libopus", "-b:a", "128k")
        .arg2_if(add_faststart, "-movflags", "+faststart")
        .arg2_if(add_cues_to_front, "-cues_to_front", "y")
        .arg(output)
        .stdin(svt_out)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg to-output")?;

    let to_output_stream = FfmpegOut::stream(to_output, "ffmpeg to-output");

    Ok(yuv_pipe.merge(svt).merge(to_output_stream))
}

pub fn default_audio_codec(
    input: &Path,
    output: &Path,
    downmix_to_stereo: bool,
    has_audio: bool,
) -> &'static str {
    // use `-c:a copy` if the extensions are the same, otherwise re-encode with opus
    match input.extension() {
        _ if downmix_to_stereo => "libopus",
        ext if ext.is_some() && ext == output.extension() => "copy",
        _ if !has_audio => "copy",
        _ => "libopus",
    }
}
