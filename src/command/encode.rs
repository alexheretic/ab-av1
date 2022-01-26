use crate::{ffmpeg::FfmpegProgress, ffprobe, svtav1};
use clap::Parser;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio_stream::StreamExt;

/// Simple invocation of ffmpeg & SvtAv1EncApp to reencode a video.
#[derive(Parser)]
pub struct EncodeArgs {
    /// Input video file.
    #[clap(short, long)]
    input: PathBuf,

    /// Encoder constant rate factor. Lower means better quality.
    #[clap(long)]
    crf: u8,

    /// Encoder preset. Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long)]
    preset: u8,

    /// Output file, by default the same as input with `.av1.mp4` extension.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

pub async fn encode(
    EncodeArgs {
        input,
        crf,
        preset,
        output,
    }: EncodeArgs,
) -> anyhow::Result<()> {
    let defaulting_output = output.is_none();
    let output = output.unwrap_or_else(|| input.with_extension("av1.mp4"));

    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg:^11}, eta {eta})")
            .progress_chars("##-")
    );
    bar.enable_steady_tick(100);
    if defaulting_output {
        bar.println(style(format!("Encoding {output:?}")).dim().to_string());
    }
    bar.set_message("encoding");

    let duration = ffprobe::probe(&input).map(|p| p.duration);
    if let Ok(d) = duration {
        bar.set_length(d.as_secs());
    }

    let mut enc = svtav1::encode(&input, crf, preset, &output)?;
    while let Some(progress) = enc.next().await {
        let FfmpegProgress { fps, time, .. } = progress?;
        if fps > 0.0 {
            bar.set_message(format!("enc {fps} fps"));
        }
        if duration.is_ok() {
            bar.set_position(time.as_secs());
        }
    }
    bar.finish();
    Ok(())
}
