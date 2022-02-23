use crate::{
    command::{args, PROGRESS_CHARS},
    console_ext::style,
    ffprobe,
    process::FfmpegOut,
    svtav1::{self},
    temporary::{self, TempKind},
};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio_stream::StreamExt;

/// Simple invocation of ffmpeg & SvtAv1EncApp to encode a video.
#[derive(Parser)]
pub struct Args {
    #[clap(flatten)]
    pub svt: args::SvtEncode,

    /// Encoder constant rate factor (1-63). Lower means better quality.
    #[clap(long)]
    pub crf: u8,

    #[clap(flatten)]
    pub encode: args::EncodeToOutput,
}

pub async fn encode(args: Args) -> anyhow::Result<()> {
    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(100);

    run(args, &bar).await
}

pub async fn run(
    Args {
        svt,
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
    let output = output.unwrap_or_else(|| default_output_from(&svt.input));
    // output is temporary until encoding has completed successfully
    temporary::add(&output, TempKind::NotKeepable);

    if defaulting_output {
        bar.println(style!("Encoding {output:?}").dim().to_string());
    }
    bar.set_message("encoding, ");

    let probe = ffprobe::probe(&svt.input);
    let svt_args = svt.to_svt_args(crf, &probe)?;
    let has_audio = probe.has_audio;
    if let Ok(d) = probe.duration {
        bar.set_length(d.as_secs());
    }

    // only downmix if achannels > 3
    let stereo_downmix = downmix_to_stereo && probe.max_audio_channels.map_or(false, |c| c > 3);
    let audio_codec = audio_codec.as_deref();
    if stereo_downmix && audio_codec == Some("copy") {
        anyhow::bail!("--stereo-downmix cannot be used with --acodec copy");
    }

    let mut enc = svtav1::encode(svt_args, &output, has_audio, audio_codec, stereo_downmix)?;
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
    let output_percent = 100.0 * output_size as f64 / fs::metadata(&svt.input).await?.len() as f64;
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
pub fn default_output_from(input: &Path) -> PathBuf {
    match input.extension().and_then(|e| e.to_str()) {
        Some(ext) => input.with_extension(format!("av1.{ext}")),
        _ => input.with_extension("av1.mp4"),
    }
}
