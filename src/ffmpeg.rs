//! ffmpeg encoding logic
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
    sync::Arc,
};
use tokio::process::Command;
use tokio_stream::Stream;

/// Exposed ffmpeg encoding args.
#[derive(Debug, Clone)]
pub struct FfmpegEncodeArgs<'a> {
    pub input: &'a Path,
    pub vcodec: Arc<str>,
    pub vfilter: Option<&'a str>,
    pub pix_fmt: PixelFormat,
    pub crf: u8,
    pub preset: Option<Arc<str>>,
    pub output_args: Vec<Arc<String>>,
    pub input_args: Vec<Arc<String>>,
}

/// Encode a sample.
pub fn encode_sample(
    FfmpegEncodeArgs {
        input,
        vcodec,
        vfilter,
        pix_fmt,
        crf,
        preset,
        output_args,
        input_args,
    }: FfmpegEncodeArgs,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
    let dest_ext = input
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("mp4");
    let pre = pre_extension_name(&vcodec);
    let mut dest = match &preset {
        Some(p) => input.with_extension(format!("{pre}.crf{crf}.{p}.{dest_ext}")),
        None => input.with_extension(format!("{pre}.crf{crf}.{dest_ext}")),
    };
    if let (Some(mut temp), Some(name)) = (temp_dir, dest.file_name()) {
        temp.push(name);
        dest = temp;
    }
    temporary::add(&dest, TempKind::Keepable);

    let enc = Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(input_args.iter().map(|a| &**a))
        .arg2("-i", input)
        .arg2("-c:v", &*vcodec)
        .args(output_args.iter().map(|a| &**a))
        .arg2(vcodec.crf_arg(), crf)
        .arg2("-pix_fmt", pix_fmt.as_str())
        .arg2_opt(vcodec.preset_arg(), preset)
        .arg2_opt("-vf", vfilter)
        .arg("-an")
        .arg(&dest)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg encode_sample")?;

    let stream = FfmpegOut::stream(enc, "ffmpeg encode_sample");
    Ok((dest, stream))
}

/// Encode to mp4 including re-encoding audio with libopus, if present.
pub fn encode(
    FfmpegEncodeArgs {
        input,
        vcodec,
        vfilter,
        pix_fmt,
        crf,
        preset,
        output_args,
        input_args,
    }: FfmpegEncodeArgs,
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
        .args(input_args.iter().map(|a| &**a))
        .arg2("-i", input)
        .arg2("-c:v", &*vcodec)
        .args(output_args.iter().map(|a| &**a))
        .arg2(vcodec.crf_arg(), crf)
        .arg2("-pix_fmt", pix_fmt.as_str())
        .arg2_opt(vcodec.preset_arg(), preset)
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
        .context("ffmpeg encode")?;

    Ok(FfmpegOut::stream(enc, "ffmpeg encode"))
}

pub fn pre_extension_name(vcodec: &str) -> &str {
    match vcodec.strip_prefix("lib").filter(|s| !s.is_empty()) {
        Some("vpx-vp9") => "vp9",
        Some(suffix) => suffix,
        _ if vcodec == "svt-av1" => "av1",
        _ => vcodec,
    }
}

trait VCodecSpecific {
    /// Arg to use preset values with, normally `-preset`.
    fn preset_arg(&self) -> &str;
    /// Arg to use crf values with, normally `-crf`.
    fn crf_arg(&self) -> &str;
}
impl VCodecSpecific for Arc<str> {
    fn preset_arg(&self) -> &str {
        match &**self {
            "libaom-av1" => "-cpu-used",
            _ => "-preset",
        }
    }

    fn crf_arg(&self) -> &str {
        if self.ends_with("vaapi") {
            // Use -qp for vaapi codecs as crf is not supported
            "-qp"
        } else {
            "-crf"
        }
    }
}
