use crate::{
    ffmpeg, ffmpeg::FfmpegProgress, ffprobe, ffprobe::Ffprobe, svtav1, temporary, vmaf,
    vmaf::VmafOut, SAMPLE_SIZE, SAMPLE_SIZE_S,
};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::fs;
use tokio_stream::StreamExt;

/// Fast VMAF score for provided AV1 re-encoding settings.
/// Uses short video samples to avoid expensive full duration encoding & vmaf calculation.
/// Also predicts encoding size & duration.
#[derive(Parser)]
pub struct SampleVmafArgs {
    /// Input video file.
    #[clap(short, long)]
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

pub async fn sample_vmaf(
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

    let bar = ProgressBar::new(SAMPLE_SIZE_S * samples * 2).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg:^12}, eta {eta})")
            .progress_chars("##-")
    );
    bar.enable_steady_tick(100);

    let mut results = Vec::new();
    for sample_n in 1..=samples {
        let sample_idx = sample_n - 1;
        bar.set_prefix(format!("Sample {sample_n}/{samples}"));
        bar.set_message("encoding");
        let sample_start =
            Duration::from_secs((duration.as_secs() - SAMPLE_SIZE_S * samples) / (samples + 1))
                * sample_n as _
                + SAMPLE_SIZE * sample_idx as _;

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
            let FfmpegProgress { time, fps, .. } = progress?;
            bar.set_position(time.as_secs() + sample_idx * SAMPLE_SIZE_S * 2);
            if fps > 0.0 {
                bar.set_message(format!("enc {fps} fps"));
            }
        }
        let encode_time = b.elapsed();
        let encoded_size = fs::metadata(&encoded_sample).await?.len();

        // calculate vmaf
        bar.set_message("vmaf running");
        let mut vmaf = vmaf::run(&sample, &encoded_sample)?;
        let mut vmaf_score = -1.0;
        while let Some(vmaf) = vmaf.next().await {
            match vmaf {
                VmafOut::Done(score) => {
                    vmaf_score = score;
                    break;
                }
                VmafOut::Progress(FfmpegProgress { time, fps, .. }) => {
                    bar.set_position(
                        SAMPLE_SIZE_S + time.as_secs() + sample_idx * SAMPLE_SIZE_S * 2,
                    );
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

        results.push(EncodeResult {
            vmaf_score,
            sample_size,
            encoded_size,
            encode_time,
        });
    }

    bar.finish();

    // encode how-to hint + predictions
    eprintln!(
        "\n{} {}",
        style("Encode with:").dim(),
        style(format!(
            "abav1 encode -i {input:?} --crf {crf} --preset {preset}"
        ))
        .dim()
        .italic()
    );
    let input_size = fs::metadata(&input).await?.len();
    let predicted_size = results.encoded_percent_size() * input_size as f64 / 100.0;
    eprint_predictions(
        predicted_size,
        results.encoded_percent_size(),
        results.estimate_encode_time(duration),
    );
    eprintln!();

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
        "{} {predicted_size} {}{encoded_percent}{} {} {predicted_encode_time}",
        style("Predicted full encode size").dim(),
        style("(").dim(),
        style(")").dim(),
        style("taking").dim()
    );
}
