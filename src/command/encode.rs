use crate::{
    command::PROGRESS_CHARS, console_ext::style, ffprobe, process::FfmpegProgress, svtav1,
    temporary,
};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::fs;
use tokio_stream::StreamExt;

/// Simple invocation of ffmpeg & SvtAv1EncApp to encode a video.
#[derive(Parser)]
pub struct Args {
    /// Input video file.
    #[clap(short, long)]
    pub input: PathBuf,

    /// Encoder constant rate factor. Lower means better quality.
    #[clap(long)]
    pub crf: u8,

    /// Encoder preset. Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long)]
    pub preset: u8,

    /// Output file, by default the same as input with `.av1.mp4` extension.
    #[clap(short, long)]
    pub output: Option<PathBuf>,
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
        input,
        crf,
        preset,
        output,
    }: Args,
    bar: &ProgressBar,
) -> anyhow::Result<()> {
    let defaulting_output = output.is_none();
    let output = output.unwrap_or_else(|| input.with_extension("av1.mp4"));
    // output is temporary until encoding has completed successfully
    temporary::add(&output);

    if defaulting_output {
        bar.println(style!("Encoding {output:?}").dim().to_string());
    }
    bar.set_message("encoding, ");

    let probe = ffprobe::probe(&input);
    let audio = probe.as_ref().map_or(true, |p| p.has_audio);
    let duration = probe.as_ref().map(|p| p.duration);
    if let Ok(d) = duration {
        bar.set_length(d.as_secs());
    }

    let mut enc = svtav1::encode(&input, crf, preset, &output, audio)?;
    while let Some(progress) = enc.next().await {
        let FfmpegProgress { fps, time, .. } = progress?;
        if fps > 0.0 {
            bar.set_message(format!("{fps} fps, "));
        }
        if duration.is_ok() {
            bar.set_position(time.as_secs());
        }
    }
    bar.finish();

    // successful encode, so don't delete it!
    temporary::unadd(&output);

    // print output info
    let output_size = fs::metadata(&output).await?.len();
    let output_percent = 100.0 * output_size as f64 / fs::metadata(&input).await?.len() as f64;
    let output_size = style(HumanBytes(output_size)).dim().bold();
    let output_percent = style!("{}%", output_percent.round()).dim().bold();
    eprintln!(
        "{} {output_size} {}{output_percent}{}",
        style("Encoded").dim(),
        style("(").dim(),
        style(")").dim(),
    );
    Ok(())
}
