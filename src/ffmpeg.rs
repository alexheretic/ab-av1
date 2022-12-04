//! ffmpeg encoding logic
use crate::{
    command::args::PixelFormat,
    process::{CommandExt, FfmpegOut},
    svtav1,
    temporary::{self, TempKind},
};
use anyhow::Context;
use std::{
    collections::HashSet,
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
    pub crf: f32,
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
    dest_ext: &str,
) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
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
        .arg("-y")
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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("ffmpeg encode_sample")?;

    let stream = FfmpegOut::stream(enc, "ffmpeg encode_sample");
    Ok((dest, stream))
}

/// Encode to output.
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
    let oargs: HashSet<_> = output_args.iter().map(|a| a.as_str()).collect();
    let output_ext = output.extension().and_then(|e| e.to_str());

    let add_faststart = output_ext == Some("mp4") && !oargs.contains("-movflags");
    let add_cues_to_front = output_ext == Some("mkv") && !oargs.contains("-cues_to_front");

    let audio_codec = audio_codec.unwrap_or_else(|| {
        svtav1::default_audio_codec(input, output, downmix_to_stereo, has_audio)
    });

    let set_ba_128k = audio_codec == "libopus" && !oargs.contains("-b:a");
    let downmix_to_stereo = downmix_to_stereo && !oargs.contains("-ac");

    let enc = Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(input_args.iter().map(|a| &**a))
        .arg("-y")
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
        .arg2_if(set_ba_128k, "-b:a", "128k")
        .arg2_if(add_faststart, "-movflags", "+faststart")
        .arg2_if(add_cues_to_front, "-cues_to_front", "y")
        .arg(output)
        .stdin(Stdio::null())
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
            "libaom-av1" | "libvpx-vp9" => "-cpu-used",
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
