//! ffmpeg encoding logic
use crate::{
    command::encoders::{svtav1::SvtEncoder, Encoder, PixelFormat, Preset},
    ffprobe::Ffprobe,
    float::TerseF32,
    process::{CommandExt, FfmpegOut},
    temporary::{self, TempKind},
};
use anyhow::{ensure, Context};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
};
use tokio::process::Command;
use tokio_stream::Stream;

/// Exposed ffmpeg encoding args.
#[derive(Debug, Clone)]
pub struct FfmpegEncodeArgs {
    pub input: Arc<PathBuf>,
    pub output: PathBuf,
    pub enc: Arc<SvtEncoder>,
    pub vcodec: Arc<str>,
    pub vfilter: Option<String>,
    pub pix_fmt: PixelFormat,
    pub output_args: Vec<Arc<String>>,
    pub input_args: Vec<Arc<String>>,
    pub video_only: bool,
}

impl FfmpegEncodeArgs {
    pub fn sample_encode_hash(&self, state: &mut impl Hasher) {
        static SVT_AV1_V: OnceLock<Vec<u8>> = OnceLock::new();

        // hashing svt-av1 version means new encoder releases will avoid old cache data
        if &*self.vcodec == "libsvtav1" {
            let svtav1_verion = SVT_AV1_V.get_or_init(|| {
                use std::process::Command;
                match Command::new("SvtAv1EncApp").arg("--version").output() {
                    Ok(out) => out.stdout,
                    _ => <_>::default(),
                }
            });
            svtav1_verion.hash(state);
        }

        // input not relevant to sample encoding
        self.vcodec.hash(state);
        self.vfilter.hash(state);
        self.pix_fmt.hash(state);
        self.output_args.hash(state);
        self.input_args.hash(state);
    }

    pub fn from_enc(
        input: Arc<PathBuf>,
        dir: Option<PathBuf>,
        enc: Arc<SvtEncoder>,
        probe: &Ffprobe,
        sample: bool,
    ) -> anyhow::Result<Self> {
        let svtav1 = enc.encoder.as_str() == "libsvtav1";
        ensure!(
            svtav1 || enc.svt_args.is_empty(),
            "--svt may only be used with svt-av1"
        );

        let preset = match &enc.preset {
            Some(Preset::Number(n)) => Some(n.to_string().into()),
            Some(Preset::Name(n)) => Some(n.clone()),
            None if svtav1 => Some("8".into()),
            None => None,
        };

        let keyint = enc.keyint(probe)?;

        let mut svtav1_params = vec![];
        if svtav1 {
            let scd = match (enc.scd, enc.keyint, keyint) {
                (Some(true), ..) | (_, None, Some(_)) => 1,
                _ => 0,
            };
            svtav1_params.push(format!("scd={scd}"));
            svtav1_params.extend(enc.svt_args.iter().map(|a| a.to_string()));
        }

        let mut args: Vec<Arc<String>> = enc
            .enc_args
            .iter()
            .flat_map(|arg| {
                if let Some((opt, val)) = arg.split_once('=') {
                    if opt == "svtav1-params" {
                        svtav1_params.push(arg.clone());
                        vec![].into_iter()
                    } else {
                        vec![opt.to_owned().into(), val.to_owned().into()].into_iter()
                    }
                } else {
                    vec![arg.clone().into()].into_iter()
                }
            })
            .collect();

        if !svtav1_params.is_empty() {
            args.push("-svtav1-params".to_owned().into());
            args.push(svtav1_params.join(":").into());
        }

        // Set preset to a default of 8 if None
        args.push("-preset".to_owned().into());
        match &enc.preset {
            Some(x) => {
                args.push(x.to_string().into());
            }
            None => args.push("8".to_owned().into()),
        };

        args.push("-crf".to_owned().into());
        args.push(enc.crf.to_string().into());

        // Set keyint/-g for all vcodecs
        if let Some(keyint) = keyint {
            if !args.iter().any(|a| &**a == "-g") {
                args.push("-g".to_owned().into());
                args.push(keyint.to_string().into());
            }
        }

        for (name, val) in enc.encoder.default_ffmpeg_args() {
            if !args.iter().any(|arg| &**arg == name) {
                args.push(name.to_string().into());
                args.push(val.to_string().into());
            }
        }

        let pix_fmt = enc.pix_format.unwrap_or(match enc.encoder.as_str() {
            vc if vc.contains("av1") => PixelFormat::Yuv420p10le,
            _ => PixelFormat::Yuv420p,
        });

        let input_args: Vec<Arc<String>> = enc
            .enc_input_args
            .iter()
            .flat_map(|arg| {
                if let Some((opt, val)) = arg.split_once('=') {
                    vec![opt.to_owned().into(), val.to_owned().into()].into_iter()
                } else {
                    vec![arg.clone().into()].into_iter()
                }
            })
            .collect();

        // ban usage of the bits we already set via other args & logic
        let reserved = HashMap::from([
            ("-c:a", " use --acodec"),
            ("-codec:a", " use --acodec"),
            ("-acodec", " use --acodec"),
            ("-i", ""),
            ("-y", ""),
            ("-n", ""),
            ("-c:v", " use --encoder"),
            ("-codec:v", " use --encoder"),
            ("-vcodec", " use --encoder"),
            ("-pix_fmt", " use --pix-format"),
            ("-vf", " use --vfilter"),
            ("-filter:v", " use --vfilter"),
        ]);
        for arg in args.iter().chain(input_args.iter()) {
            if let Some(hint) = reserved.get(arg.as_str()) {
                anyhow::bail!("Encoder argument `{arg}` not allowed{hint}");
            }
        }

        let mut output = temporary::process_dir(dir);
        output.push(match sample {
            true => match &enc.preset {
                Some(p) => input.with_extension(format!(
                    "{}.crf{}.{p}.{}",
                    enc.ext,
                    enc.crf,
                    input.extension().unwrap().to_str().unwrap()
                )),
                None => input.with_extension(format!(
                    "{}.crf{}.8.{}",
                    enc.ext,
                    enc.crf,
                    input.extension().unwrap().to_str().unwrap()
                )),
            },
            false => input.with_extension(format!(
                "{}.{}",
                enc.ext,
                input.extension().unwrap().to_str().unwrap()
            )),
        });

        Ok(FfmpegEncodeArgs {
            input,
            output,
            enc: enc.clone(),
            vcodec: enc.encoder.0.clone(),
            pix_fmt,
            vfilter: enc.vfilter.clone(),
            output_args: args,
            input_args,
            video_only: false,
        })
    }

    /// Encode to output.
    pub fn encode(
        &self,
        has_audio: bool,
        audio_codec: Option<&str>,
        downmix_to_stereo: bool,
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<FfmpegOut>>> {
        let oargs: HashSet<_> = self.output_args.iter().map(|a| a.as_str()).collect();
        let output_ext = self.output.extension().and_then(|e| e.to_str());

        let add_faststart = output_ext == Some("mp4") && !oargs.contains("-movflags");
        let add_cues_to_front =
            matches!(output_ext, Some("mkv") | Some("webm")) && !oargs.contains("-cues_to_front");

        let audio_codec = audio_codec.unwrap_or(if downmix_to_stereo && has_audio {
            "libopus"
        } else {
            "copy"
        });

        let set_ba_128k = audio_codec == "libopus" && !oargs.contains("-b:a");
        let downmix_to_stereo = downmix_to_stereo && !oargs.contains("-ac");
        let map = match self.video_only {
            true => "0:v:0",
            false => "0",
        };

        let enc = Command::new("ffmpeg")
            .kill_on_drop(true)
            .args(self.input_args.iter().map(|a| &**a))
            .arg("-y")
            .arg2("-i", self.input.clone())
            .arg2("-map", map)
            .arg2("-c:v", "copy")
            .arg2("-c:v:0", &*self.vcodec)
            .args(self.output_args.iter().map(|a| &**a))
            .arg2("-pix_fmt", self.pix_fmt.as_str())
            .arg2_opt("-vf", self.vfilter.clone())
            .arg2("-c:s", "copy")
            .arg2("-c:a", audio_codec)
            .arg2_if(downmix_to_stereo, "-ac", 2)
            .arg2_if(set_ba_128k, "-b:a", "128k")
            .arg2_if(add_faststart, "-movflags", "+faststart")
            .arg2_if(add_cues_to_front, "-cues_to_front", "y")
            .arg(self.output.clone())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("ffmpeg encode")?;

        Ok(FfmpegOut::stream(enc, "ffmpeg encode"))
    }
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
        if &**self == "librav1e" || self.ends_with("_vaapi") {
            "-qp"
        } else if self.ends_with("_nvenc") {
            "-cq"
        } else if self.ends_with("_qsv") {
            "-global_quality"
        } else {
            "-crf"
        }
    }
}
