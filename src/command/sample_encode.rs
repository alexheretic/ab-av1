use crate::{
    ffprobe, ffprobe::Ffprobe, sample, sample::FfmpegProgress, svtav1, temporary, vmaf,
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

/// Encode short video samples of an input using provided **crf** & **preset**.
/// This is much quicker than full encode/vmaf run.
///
/// Outputs:
/// * Mean sample VMAF score
/// * Predicted full encode size
/// * Predicted full encode time
#[derive(Parser)]
pub struct SampleEncodeArgs {
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

    /// Keep temporary files after exiting.
    #[clap(long)]
    pub keep: bool,

    /// Stdout message format `human` or `json`.
    #[clap(long, arg_enum, default_value_t = StdoutFormat::Human)]
    stdout_format: StdoutFormat,
}

pub async fn sample_encode(
    SampleEncodeArgs {
        input,
        crf,
        preset,
        samples,
        keep,
        stdout_format,
    }: SampleEncodeArgs,
) -> anyhow::Result<()> {
    let Ffprobe { duration, .. } = ffprobe::probe(&input)?;
    let samples = samples.min(duration.as_secs() / SAMPLE_SIZE_S);

    let bar = ProgressBar::new(SAMPLE_SIZE_S * samples * 2).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg:13} eta {eta})")
            .progress_chars("##-")
    );
    bar.enable_steady_tick(100);

    let mut results = Vec::new();
    for sample_n in 1..=samples {
        let sample_idx = sample_n - 1;
        bar.set_prefix(format!("Sample {sample_n}/{samples}"));

        let sample_start =
            Duration::from_secs((duration.as_secs() - SAMPLE_SIZE_S * samples) / (samples + 1))
                * sample_n as _
                + SAMPLE_SIZE * sample_idx as _;

        // cut sample
        bar.set_message("sampling,");
        let sample = sample::copy(&input, sample_start).await?;
        let sample_size = fs::metadata(&sample).await?.len();

        // encode sample
        bar.set_message("encoding,");
        let b = Instant::now();
        let (encoded_sample, mut output) = svtav1::encode_ivf(&sample, crf, preset)?;
        while let Some(progress) = output.next().await {
            let FfmpegProgress { time, fps, .. } = progress?;
            bar.set_position(time.as_secs() + sample_idx * SAMPLE_SIZE_S * 2);
            if fps > 0.0 {
                bar.set_message(format!("enc {fps} fps,"));
            }
        }
        let encode_time = b.elapsed();
        let encoded_size = fs::metadata(&encoded_sample).await?.len();

        // calculate vmaf
        bar.set_message("vmaf running,");
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
                        bar.set_message(format!("vmaf {fps} fps,"));
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

        if !keep {
            temporary::clean().await;
        }
    }

    bar.finish();

    // encode how-to hint + predictions
    eprintln!(
        "\n{} {}\n",
        style("Encode with:").dim(),
        style(format!(
            "ab-av1 encode -i {input:?} --crf {crf} --preset {preset}"
        ))
        .dim()
        .italic()
    );
    // stdout result
    let input_size = fs::metadata(&input).await?.len();
    let predicted_size = results.encoded_percent_size() * input_size as f64 / 100.0;
    stdout_format.print_result(
        results.mean_vmaf(),
        predicted_size as _,
        results.encoded_percent_size(),
        results.estimate_encode_time(duration),
    );
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

#[derive(Debug, Clone, Copy, clap::ArgEnum)]
pub enum StdoutFormat {
    Human,
    Json,
}

impl StdoutFormat {
    fn print_result(self, vmaf: f32, size: u64, percent: f64, time: Duration) {
        match self {
            Self::Human => {
                let vmaf = match vmaf {
                    v if v >= 95.0 => style(v).bold().green(),
                    v if v < 80.0 => style(v).bold().red(),
                    v => style(v).bold(),
                };
                let percent = percent.round();
                let size = match size {
                    v if percent < 80.0 => style(HumanBytes(v)).bold().green(),
                    v if percent >= 100.0 => style(HumanBytes(v)).bold().red(),
                    v => style(HumanBytes(v)).bold(),
                };
                let percent = match percent {
                    v if v < 80.0 => style(format!("{}%", v)).bold().green(),
                    v if v >= 100.0 => style(format!("{}%", v)).bold().red(),
                    v => style(format!("{}%", v)).bold(),
                };
                let time = style(HumanDuration(time)).bold();
                println!(
                    "VMAF {vmaf:.2} predicted full encode size {size} ({percent}) taking {time}"
                );
            }
            Self::Json => {
                let json = serde_json::json!({
                    "vmaf": vmaf,
                    "predicted_encode_size": size,
                    "predicted_encode_percent": percent,
                    "predicted_encode_seconds": time.as_secs(),
                });
                println!("{}", serde_json::to_string(&json).unwrap());
            }
        }
    }
}
