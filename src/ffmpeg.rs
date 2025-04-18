//! ffmpeg encoding logic
use crate::{
    command::args::PixelFormat,
    float::TerseF32,
    process::{CommandExt, FfmpegOut, FfmpegOutStream},
    temporary::{self, TempKind},
};
use anyhow::Context;
use log::debug;
use std::{
    collections::HashSet,
    fmt::Write,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock},
};
use tokio::process::Command;

/// Exposed ffmpeg encoding args.
#[derive(Debug, Clone)]
pub struct FfmpegEncodeArgs<'a> {
    pub input: &'a Path,
    pub vcodec: Arc<str>,
    pub vfilter: Option<&'a str>,
    pub pix_fmt: Option<PixelFormat>,
    pub crf: f32,
    pub preset: Option<Arc<str>>,
    pub output_args: Vec<Arc<String>>,
    pub input_args: Vec<Arc<String>>,
    pub video_only: bool,
}

impl FfmpegEncodeArgs<'_> {
    pub fn sample_encode_hash(&self, state: &mut impl Hasher) {
        static SVT_AV1_V: LazyLock<Vec<u8>> = LazyLock::new(|| {
            std::process::Command::new("SvtAv1EncApp")
                .arg("--version")
                .output()
                .map(|o| o.stdout)
                .unwrap_or_default()
        });

        // hashing svt-av1 version means new encoder releases will avoid old cache data
        if &*self.vcodec == "libsvtav1" {
            SVT_AV1_V.hash(state);
        }

        // input not relevant to sample encoding
        self.vcodec.hash(state);
        self.vfilter.hash(state);
        self.pix_fmt.hash(state);
        self.crf.to_bits().hash(state);
        self.preset.hash(state);
        self.output_args.hash(state);
        self.input_args.hash(state);
    }
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
        video_only: _,
    }: FfmpegEncodeArgs,
    temp_dir: Option<PathBuf>,
    dest_ext: &str,
) -> anyhow::Result<(PathBuf, FfmpegOutStream)> {
    let pre = pre_extension_name(&vcodec);
    let crf_str = format!("{}", TerseF32(crf)).replace('.', "_");
    let dest_file_name = match &preset {
        Some(p) => input.with_extension(format!("{pre}.crf{crf_str}.{p}.{dest_ext}")),
        None => input.with_extension(format!("{pre}.crf{crf_str}.{dest_ext}")),
    };
    let dest_file_name = dest_file_name.file_name().unwrap();
    let mut dest = temporary::process_dir(temp_dir);
    dest.push(dest_file_name);

    temporary::add(&dest, TempKind::Keepable);

    let mut cmd = Command::new("ffmpeg");
    cmd.kill_on_drop(true)
        .arg("-y")
        .args(input_args.iter().map(|a| &**a))
        .arg2("-i", input)
        .arg2("-c:v", &*vcodec)
        .args(output_args.iter().map(|a| &**a))
        .arg2(vcodec.crf_arg(), crf)
        .arg2_opt("-pix_fmt", pix_fmt.map(|v| v.as_str()))
        .arg2_opt(vcodec.preset_arg(), preset)
        .arg2_opt("-vf", vfilter)
        .arg("-an")
        .arg(&dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let cmd_str = cmd.to_cmd_str();
    debug!("cmd `{cmd_str}`");

    let enc = cmd.spawn().context("ffmpeg encode_sample")?;

    let stream = FfmpegOut::stream(enc, "ffmpeg encode_sample", cmd_str);
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
        video_only,
    }: FfmpegEncodeArgs,
    output: &Path,
    has_audio: bool,
    audio_codec: Option<&str>,
    downmix_to_stereo: bool,
) -> anyhow::Result<FfmpegOutStream> {
    let oargs: HashSet<_> = output_args.iter().map(|a| a.as_str()).collect();
    let output_ext = output.extension().and_then(|e| e.to_str());

    let add_faststart = output_ext == Some("mp4") && !oargs.contains("-movflags");
    let matroska = matches!(output_ext, Some("mkv") | Some("webm"));
    let add_cues_to_front = matroska && !oargs.contains("-cues_to_front");

    let audio_codec = audio_codec.unwrap_or(if downmix_to_stereo && has_audio {
        "libopus"
    } else {
        "copy"
    });

    let set_ba_128k = audio_codec == "libopus" && !oargs.contains("-b:a");
    let downmix_to_stereo = downmix_to_stereo && !oargs.contains("-ac");
    let map = match video_only {
        true => "0:v:0",
        false => "0",
    };
    let mut metadata = format!(
        "AB_AV1_FFMPEG_ARGS=-c:v {vcodec} {} {crf}",
        vcodec.crf_arg()
    );
    if let Some(preset) = &preset {
        write!(&mut metadata, " {} {preset}", vcodec.preset_arg()).unwrap();
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.kill_on_drop(true)
        .args(input_args.iter().map(|a| &**a))
        .arg("-y")
        .arg2("-i", input)
        .arg2("-map", map)
        .arg2("-c:v", "copy")
        .arg2("-c:v:0", &*vcodec)
        .arg2("-metadata:s:v:0", metadata)
        .arg2("-c:a", audio_codec)
        .arg2("-c:s", "copy")
        .args(output_args.iter().map(|a| &**a))
        .arg2(vcodec.crf_arg(), crf)
        .arg2_opt("-pix_fmt", pix_fmt.map(|v| v.as_str()))
        .arg2_opt(vcodec.preset_arg(), preset)
        .arg2_opt("-vf", vfilter)
        .arg_if(matroska, "-dn") // "Only audio, video, and subtitles are supported for Matroska"
        .arg2_if(downmix_to_stereo, "-ac", 2)
        .arg2_if(set_ba_128k, "-b:a", "128k")
        .arg2_if(add_faststart, "-movflags", "+faststart")
        .arg2_if(add_cues_to_front, "-cues_to_front", "y")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let cmd_str = cmd.to_cmd_str();
    debug!("cmd `{cmd_str}`");

    let enc = cmd.spawn().context("ffmpeg encode")?;

    Ok(FfmpegOut::stream(enc, "ffmpeg encode", cmd_str))
}

pub fn pre_extension_name(vcodec: &str) -> &str {
    match vcodec.strip_prefix("lib").filter(|s| !s.is_empty()) {
        Some("svtav1") => "av1",
        Some("vpx-vp9") => "vp9",
        Some(suffix) => suffix,
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
            "librav1e" => "-speed",
            _ => "-preset",
        }
    }

    fn crf_arg(&self) -> &str {
        // use crf-like args to support encoders that don't have crf
        match &**self {
            // https://ffmpeg.org//ffmpeg-codecs.html#librav1e
            // https://github.com/fraunhoferhhi/vvenc/wiki/FFmpeg-Integration#fix-qp-mode-constant-quality-mode
            "librav1e" | "libvvenc" => "-qp",
            "mpeg2video" => "-q",
            // https://ffmpeg.org//ffmpeg-codecs.html#VAAPI-encoders
            e if e.ends_with("_vaapi") => "-q",
            e if e.ends_with("_vulkan") => "-qp",
            e if e.ends_with("_nvenc") => "-cq",
            // https://ffmpeg.org//ffmpeg-codecs.html#QSV-Encoders
            e if e.ends_with("_qsv") => "-global_quality",
            _ => "-crf",
        }
    }
}
