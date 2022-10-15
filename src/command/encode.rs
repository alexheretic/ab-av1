use crate::{
    command::{
        args::{self, EncoderArgs},
        PROGRESS_CHARS,
    },
    console_ext::style,
    ffmpeg, ffprobe,
    process::FfmpegOut,
    svtav1::{self},
    temporary::{self, TempKind},
};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use std::{path::PathBuf, time::Duration};
use tokio::fs;
use tokio_stream::StreamExt;

/// Simple invocation of ffmpeg & SvtAv1EncApp to encode a video.
#[derive(Parser)]
pub struct Args {
    #[clap(flatten)]
    pub args: args::Encode,

    /// Encoder constant rate factor (1-63). Lower means better quality.
    #[arg(long)]
    pub crf: u8,

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

    run(args, &bar).await
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
            },
    }: Args,
    bar: &ProgressBar,
) -> anyhow::Result<()> {
    let defaulting_output = output.is_none();
    let probe = ffprobe::probe(&args.input);
    let duration = probe.duration.as_ref().unwrap();
    let output = output.unwrap_or_else(|| default_output_from(&args, duration.is_zero()));
    // output is temporary until encoding has completed successfully
    temporary::add(&output, TempKind::NotKeepable);

    if defaulting_output {
        let out = shell_escape::escape(output.display().to_string().into());
        bar.println(style!("Encoding {out}").dim().to_string());
    }
    bar.set_message("encoding, ");

    let enc_args = args.to_encoder_args(crf, &probe)?;
    let has_audio = probe.has_audio;
    bar.set_length(duration.as_secs());

    // only downmix if achannels > 3
    let stereo_downmix = downmix_to_stereo && probe.max_audio_channels.map_or(false, |c| c > 3);
    let audio_codec = audio_codec.as_deref();
    if stereo_downmix && audio_codec == Some("copy") {
        anyhow::bail!("--stereo-downmix cannot be used with --acodec copy");
    }

    let mut enc = match enc_args {
        EncoderArgs::SvtAv1(args) => {
            let enc = svtav1::encode(args, &output, has_audio, audio_codec, stereo_downmix)?;
            futures::StreamExt::boxed_local(enc)
        }
        EncoderArgs::Ffmpeg(args) => {
            let enc = ffmpeg::encode(args, &output, has_audio, audio_codec, stereo_downmix)?;
            futures::StreamExt::boxed_local(enc)
        }
    };
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

/// * input: vid.ext -> output: vid.av1.ext
pub fn default_output_from(enc: &args::Encode, is_image: bool) -> PathBuf {
    let pre = ffmpeg::pre_extension_name(enc.encoder.as_str());

    match enc
        .input
        .extension()
        .and_then(|e| e.to_str())
        // don't use extensions that won't work
        .filter(|e| *e != "avi" && *e != "y4m" && *e != "ivf")
    {
        Some(ext) => {
            let ext = if is_image { "avif" } else { ext };
            enc.input.with_extension(format!("{pre}.{ext}"))
        }
        _ => enc.input.with_extension(format!("{pre}.mp4")),
    }
}
