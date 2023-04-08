use crate::{
    command::{
        args::{self, Encoder},
        PROGRESS_CHARS,
    },
    console_ext::style,
    ffmpeg,
    ffprobe::{self, Ffprobe},
    process::FfmpegOut,
    temporary::{self, TempKind},
};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::fs;
use tokio_stream::StreamExt;

/// Invoke ffmpeg to encode a video or image.
#[derive(Parser)]
#[group(skip)]
pub struct Args {
    #[clap(flatten)]
    pub args: args::Encode,

    /// Encoder constant rate factor (1-63). Lower means better quality.
    #[arg(long)]
    pub crf: f32,

    #[clap(flatten)]
    pub encode: args::EncodeToOutput,
}

pub async fn encode(args: Args) -> anyhow::Result<()> {
    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(Duration::from_millis(100));

    let probe = ffprobe::probe(&args.args.input);
    run(args, probe.into(), &bar).await
}

pub async fn run(
    Args {
        args,
        crf,
        encode:
            args::EncodeToOutput {
                output,
                audio_codec,
                downmix_to_stereo,
                video_only,
            },
    }: Args,
    probe: Arc<Ffprobe>,
    bar: &ProgressBar,
) -> anyhow::Result<()> {
    let defaulting_output = output.is_none();
    // let probe = ffprobe::probe(&args.input);
    let output =
        output.unwrap_or_else(|| default_output_name(&args.input, &args.encoder, probe.is_image));
    // output is temporary until encoding has completed successfully
    temporary::add(&output, TempKind::NotKeepable);

    if defaulting_output {
        let out = shell_escape::escape(output.display().to_string().into());
        bar.println(style!("Encoding {out}").dim().to_string());
    }
    bar.set_message("encoding, ");

    let mut enc_args = args.to_encoder_args(crf, &probe)?;
    enc_args.video_only = video_only;
    let has_audio = probe.has_audio;
    if let Ok(d) = probe.duration {
        bar.set_length(d.as_secs().max(1));
    }

    // only downmix if achannels > 3
    let stereo_downmix = downmix_to_stereo && probe.max_audio_channels.map_or(false, |c| c > 3);
    let audio_codec = audio_codec.as_deref();
    if stereo_downmix && audio_codec == Some("copy") {
        anyhow::bail!("--stereo-downmix cannot be used with --acodec copy");
    }

    let mut enc = ffmpeg::encode(enc_args, &output, has_audio, audio_codec, stereo_downmix)?;

    let mut stream_sizes = None;
    while let Some(progress) = enc.next().await {
        match progress? {
            FfmpegOut::Progress { fps, time, .. } => {
                if fps > 0.0 {
                    bar.set_message(format!("{fps} fps, "));
                }
                if probe.duration.is_ok() {
                    bar.set_position(time.as_secs());
                }
            }
            FfmpegOut::StreamSizes {
                video,
                audio,
                subtitle,
                other,
            } => stream_sizes = Some((video, audio, subtitle, other)),
        }
    }
    bar.finish();

    // successful encode, so don't delete it!
    temporary::unadd(&output);

    // print output info
    let output_size = fs::metadata(&output).await?.len();
    let output_percent = 100.0 * output_size as f64 / fs::metadata(&args.input).await?.len() as f64;
    let output_size = style(HumanBytes(output_size)).dim().bold();
    let output_percent = style!("{}%", output_percent.round()).dim().bold();
    eprint!(
        "{} {output_size} {}{output_percent}",
        style("Encoded").dim(),
        style("(").dim(),
    );
    if let Some((video, audio, subtitle, other)) = stream_sizes {
        if audio > 0 || subtitle > 0 || other > 0 {
            for (label, size) in [
                ("video:", video),
                ("audio:", audio),
                ("subs:", subtitle),
                ("other:", other),
            ] {
                if size > 0 {
                    let size = style(HumanBytes(size)).dim();
                    eprint!("{} {}{size}", style(",").dim(), style(label).dim(),);
                }
            }
        }
    }
    eprintln!("{}", style(")").dim());

    Ok(())
}

/// * vid.mp4 -> "mp4"
/// * vid.??? -> "mkv"
/// * image.??? -> "avif"
pub fn default_output_ext(input: &Path, is_image: bool) -> &'static str {
    if is_image {
        return "avif";
    }
    match input.extension().and_then(|e| e.to_str()) {
        Some("mp4") => "mp4",
        _ => "mkv",
    }
}

/// E.g. vid.mkv -> "vid.av1.mkv"
pub fn default_output_name(input: &Path, encoder: &Encoder, is_image: bool) -> PathBuf {
    let pre = ffmpeg::pre_extension_name(encoder.as_str());
    let ext = default_output_ext(input, is_image);
    input.with_extension(format!("{pre}.{ext}"))
}
