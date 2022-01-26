mod ffmpeg;
mod ffprobe;
mod svtav1;
mod temporary;
mod vmaf;

use crate::{ffmpeg::FfmpegProgress, ffprobe::Ffprobe, vmaf::VmafOut};
use anyhow::anyhow;
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{fs, signal};
use tokio_stream::StreamExt;

const SAMPLE_SIZE_S: u64 = 20;
const SAMPLE_SIZE: Duration = Duration::from_secs(SAMPLE_SIZE_S);

#[derive(Parser)]
#[clap(version, about)]
struct Args {
    #[clap(subcommand)]
    action: Action,

    /// Keep temporary files after exiting.
    #[clap(long)]
    keep: bool,
}

#[derive(clap::Subcommand)]
enum Action {
    SampleVmaf(SampleVmafArgs),
}

/// Fast calculation of VMAF score for AV1 re-encoding settings using short samples.
#[derive(Parser)]
struct SampleVmafArgs {
    /// Input video file.
    #[clap(short)]
    input: PathBuf,

    /// Encoder constant rate factor. Lower means better quality.
    #[clap(long)]
    crf: u8,

    /// Encoder preset. Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long)]
    preset: u8,

    /// Number of 20s samples.
    #[clap(long, default_value_t = 3)]
    samples: u64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let Args {
        action: Action::SampleVmaf(vmaf_args),
        keep,
    } = Args::parse();

    let out = tokio::select! {
        r = sample_vmaf(vmaf_args) => r,
        _ = signal::ctrl_c() => Err(anyhow!("ctrl_c")),
    };

    if !keep {
        temporary::clean().await;
    }

    out
}

async fn sample_vmaf(
    SampleVmafArgs {
        input,
        crf,
        preset,
        samples,
        ..
    }: SampleVmafArgs,
) -> anyhow::Result<()> {
    let Ffprobe { duration, .. } = ffprobe::probe(&input)?;
    let samples = samples.min(duration.as_secs() / SAMPLE_SIZE_S);

    let bar = ProgressBar::new(samples * 2).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg}, eta {eta})")
            .progress_chars("##-")
    );
    bar.enable_steady_tick(100);
    bar.set_message("encoding");

    let mut results = Vec::new();
    for sample_n in 1..=samples {
        bar.set_prefix(format!("Sample {sample_n}/{samples}"));
        let sample_start =
            Duration::from_secs((duration.as_secs() - SAMPLE_SIZE_S * samples) / (samples + 1))
                * sample_n as _
                + SAMPLE_SIZE * (sample_n - 1) as _;

        // cut sample
        let (sample, mut output) = ffmpeg::cut_sample(&input, sample_start)?;
        temporary::add(&sample);
        output.next().await.map(Err).unwrap_or(Ok(()))?;
        let sample_size = fs::metadata(&sample).await?.len();

        // encode sample
        let b = Instant::now();
        let (encoded_sample, mut output) = svtav1::encode_ivf(&sample, crf, preset)?;
        temporary::add(&encoded_sample);
        while let Some(progress) = output.next().await {
            let fps = progress?.fps;
            if fps > 0.0 {
                bar.set_message(format!("encoding {fps} fps"));
            }
        }
        let encode_time = b.elapsed();
        bar.inc(1);
        let encoded_size = fs::metadata(&encoded_sample).await?.len();

        // calculate vmaf
        let mut vmaf = vmaf::run(&sample, &encoded_sample)?;
        let mut vmaf_score = -1.0;
        while let Some(vmaf) = vmaf.next().await {
            match vmaf {
                VmafOut::Done(score) => {
                    vmaf_score = score;
                    break;
                }
                VmafOut::Progress(FfmpegProgress { fps, .. }) => {
                    if fps > 0.0 {
                        bar.set_message(format!("vmaf {fps} fps"));
                    }
                }
                VmafOut::Err(e) => return Err(e),
            }
        }

        bar.println(
            style(format!(
                "- Sample {sample_n} ({:.0}%) vmaf {vmaf_score:.2}",
                100.0 * encoded_size as f32 / sample_size as f32
            ))
            .dim()
            .to_string(),
        );
        bar.inc(1);

        results.push(EncodeResult {
            vmaf_score,
            sample_size,
            encoded_size,
            encode_time,
        });
    }

    bar.finish();

    let input_size = fs::metadata(&input).await?.len();
    let predicted_size = results.encoded_percent_size() * input_size as f64 / 100.0;
    eprint_predictions(
        predicted_size,
        results.encoded_percent_size(),
        results.estimate_encode_time(duration),
    );

    // temporary encode how-to hint
    eprintln!("{}", style(format!("ffmpeg -loglevel panic -i {input:?} -strict -1 -f yuv4mpegpipe - |\n  \
        SvtAv1EncApp -i stdin -b stdout --crf {crf} --progress 0 --preset {preset} |\n  \
        ffmpeg -i - -i {input:?} -map 0:v -map 1:a:0 -c:v copy -c:a libopus -movflags +faststart out.mp4\n")).dim());

    // finally print the mean sample vmaf
    println!("{}", results.mean_vmaf());
    Ok(())
}

struct EncodeResult {
    sample_size: u64,
    encoded_size: u64,
    vmaf_score: f32,
    encode_time: Duration,
}

trait EncodeResults {
    fn encoded_percent_size(&self) -> f64;
    fn mean_vmaf(&self) -> f32;
    fn estimate_encode_time(&self, input_duration: Duration) -> Duration;
}
impl EncodeResults for Vec<EncodeResult> {
    fn encoded_percent_size(&self) -> f64 {
        let encoded = self.iter().map(|r| r.encoded_size).sum::<u64>() as f64;
        let sample = self.iter().map(|r| r.sample_size).sum::<u64>() as f64;
        encoded * 100.0 / sample
    }

    fn mean_vmaf(&self) -> f32 {
        self.iter().map(|r| r.vmaf_score).sum::<f32>() / self.len() as f32
    }

    fn estimate_encode_time(&self, input_duration: Duration) -> Duration {
        let sample_factor =
            input_duration.as_secs_f64() / (SAMPLE_SIZE_S as f64 * self.len() as f64);

        let sample_encode_time: f64 = self.iter().map(|r| r.encode_time.as_secs_f64()).sum();

        let estimate = Duration::from_secs_f64(sample_encode_time * sample_factor);
        if estimate < Duration::from_secs(1) {
            estimate
        } else {
            Duration::from_secs(estimate.as_secs())
        }
    }
}

fn eprint_predictions(size: f64, percent: f64, time: Duration) {
    let predicted_size = style(HumanBytes(size as _)).dim().bold();
    let encoded_percent = style(format!("{}%", percent.round())).dim().bold();
    let predicted_encode_time = style(HumanDuration(time)).dim().bold();
    eprintln!(
        "\n{} {predicted_size} {}{encoded_percent}{} {} {predicted_encode_time}\n",
        style("Predicted full encode size").dim(),
        style("(").dim(),
        style(")").dim(),
        style("taking").dim()
    );
}
