//! ffmpeg encoding logic
use crate::{
    command::args::PixelFormat,
    float::TerseF32,
    process::{CommandExt, FfmpegOut, FfmpegOutStream},
    temporary::{self, TempKind},
};
use anyhow::Context;
use bstr::ByteSlice;
use log::debug;
use std::{
    collections::HashSet,
    fmt::Write,
    hash::{Hash, Hasher},
    io::Read,
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
        static SVT_AV1_V: LazyLock<String> = LazyLock::new(|| {
            ffmpeg_svtav1_version()
                .inspect_err(|e| debug!("read_ffmpeg_svtav1_version: {e}"))
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
    let mut dest = temporary::process_dir(temp_dir)?;
    dest.push(dest_file_name);

    temporary::add(&dest, TempKind::Keepable);

    let mut cmd = Command::new("ffmpeg");
    cmd.kill_on_drop(true)
        .arg("-y")
        .args(input_args.iter().map(|a| &**a))
        .arg2("-i", input)
        .arg2("-c:v", &*vcodec)
        .args(output_args.iter().map(|a| &**a))
        // Avoid dropping or duplicating frames as this may negatively affect input/output analysis
        .arg2("-fps_mode", "passthrough")
        .arg2(vcodec.crf_arg(), vcodec.crf(crf))
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
    // This doesn't seem to work on .mp4 files
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
        .arg2("-metadata", metadata)
        .arg2("-c:a", audio_codec)
        .arg2("-c:s", "copy")
        .args(output_args.iter().map(|a| &**a))
        .arg2(vcodec.crf_arg(), vcodec.crf(crf))
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
    /// crf value to pass to ffmpeg.
    fn crf(&self, crf: f32) -> f32;
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
            "hevc_videotoolbox" => "-q:v",
            // https://ffmpeg.org//ffmpeg-codecs.html#VAAPI-encoders
            e if e.ends_with("_vaapi") => "-q",
            e if e.ends_with("_vulkan") => "-qp",
            e if e.ends_with("_nvenc") => "-cq",
            // https://ffmpeg.org//ffmpeg-codecs.html#QSV-Encoders
            e if e.ends_with("_qsv") => "-global_quality",
            _ => "-crf",
        }
    }

    fn crf(&self, crf: f32) -> f32 {
        match &**self {
            // ffmpeg svt-av1 crf above 63 don't work, but up to 70 does work in -svtav1-params
            "libsvtav1" => crf.min(63.0),
            _ => crf,
        }
    }
}

pub fn remove_arg(args: &mut Vec<Arc<String>>, arg: &'static str) {
    let mut retain_next = true;
    args.retain(|a| {
        if **a == arg {
            retain_next = false;
            false
        } else if !retain_next {
            retain_next = true;
            false
        } else {
            true
        }
    });
}

fn ffmpeg_svtav1_version() -> anyhow::Result<String> {
    let mut ffmpeg = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "info",
            "-f",
            "lavfi",
            "-i",
            "color",
            "-frames:v",
            "1",
            "-c:v",
            "libsvtav1",
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    // buffer at least 2x the size of the expected prefix + version
    // as we'll lose the first half once we start running out of space
    // Example: "SVT-AV1 Encoder Lib v123.456.678"
    const BUF_QUARTER_LEN: usize = 64;

    let mut buf = [0; 4 * BUF_QUARTER_LEN];
    let mut read_up_to = 0;
    let mut stderr = ffmpeg.stderr.take().context("stderr")?;

    std::thread::spawn(move || _ = ffmpeg.wait());

    loop {
        let n = stderr.read(&mut buf[read_up_to..])?;
        anyhow::ensure!(n != 0, "EOF: No version string found");

        read_up_to += n;

        if let Some(idx) = buf[..read_up_to].find_iter("SVT-AV1 Encoder Lib v").next()
            && let Some(last_byte) = buf[..read_up_to].last()
            // the last byte should be past of the version bytes
            && !matches!(*last_byte, b'0'..=b'9' | b'.' | b'v')
        {
            let ver: Vec<_> = buf[idx + "SVT-AV1 Encoder Lib v".len()..read_up_to]
                .iter()
                .copied()
                .take_while(|b| matches!(b, b'0'..=b'9' | b'.'))
                .collect();

            return Ok(String::try_from(ver)?);
        }

        if read_up_to > 3 * BUF_QUARTER_LEN {
            buf.rotate_left(2 * BUF_QUARTER_LEN);
            read_up_to -= 2 * BUF_QUARTER_LEN;
        }
    }
}
