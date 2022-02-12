use crate::{
    command::{args, PROGRESS_CHARS},
    console_ext::style,
    ffprobe,
    process::FfmpegProgress,
    sample,
    svtav1::{self, SvtArgs},
    temporary, vmaf,
    vmaf::VmafOut,
    SAMPLE_SIZE, SAMPLE_SIZE_S,
};
use anyhow::ensure;
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use std::time::{Duration, Instant};
use tokio::fs;
use tokio_stream::StreamExt;

/// Encode & analyse input samples to predict how a full encode would go.
/// This is much quicker than a full encode/vmaf run.
///
/// Outputs:
/// * Mean sample VMAF score
/// * Predicted full encode size
/// * Predicted full encode time
#[derive(Parser, Clone)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub svt: args::SvtEncode,

    /// Encoder constant rate factor (1-63). Lower means better quality.
    #[clap(long)]
    pub crf: u8,

    /// Number of 20s samples to use across the input video.
    /// More samples take longer but may provide a more accurate result.
    #[clap(long, default_value_t = 3)]
    pub samples: u64,

    /// Keep temporary files after exiting.
    #[clap(long)]
    pub keep: bool,

    /// Stdout message format `human` or `json`.
    #[clap(long, arg_enum, default_value_t = StdoutFormat::Human)]
    pub stdout_format: StdoutFormat,

    #[clap(flatten)]
    pub vmaf: args::Vmaf,
}

pub async fn sample_encode(args: Args) -> anyhow::Result<()> {
    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg:13} eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(100);

    run(args, bar).await?;
    Ok(())
}

pub async fn run(
    Args {
        svt,
        crf,
        samples,
        keep,
        stdout_format,
        vmaf,
    }: Args,
    bar: ProgressBar,
) -> anyhow::Result<Output> {
    let input = &svt.input;
    let probe = ffprobe::probe(input);
    let svt_args = svt.to_svt_args(crf, &probe)?;
    let duration = probe.duration?;

    let (samples, full_pass) = {
        let samples = samples.min(duration.as_secs() / SAMPLE_SIZE_S);
        if SAMPLE_SIZE * samples.max(1) as _ >= duration {
            // if the input is lower duration than samples just encode the whole thing
            (1, true)
        } else {
            (samples, false)
        }
    };

    bar.set_length(SAMPLE_SIZE_S * samples * 2);

    let mut results = Vec::new();
    for sample_n in 1..=samples {
        let sample_idx = sample_n - 1;
        bar.set_prefix(format!("Sample {sample_n}/{samples}"));

        let (sample, sample_size) = if full_pass {
            // use the entire video as a single sample
            let input_size = fs::metadata(input).await?.len();
            (svt.input.clone(), input_size)
        } else {
            let sample_start =
                Duration::from_secs((duration.as_secs() - SAMPLE_SIZE_S * samples) / (samples + 1))
                    * sample_n as _
                    + SAMPLE_SIZE * sample_idx as _;

            // cut sample
            bar.set_message("sampling,");
            let sample = sample::copy(input, sample_start).await?;
            let sample_size = fs::metadata(&sample).await?.len();
            ensure!(
                // ffmpeg copy may fail sucessfully and give us a small/empty output
                sample_size > 1024,
                "ffmpeg copy failed: encoded sample too small"
            );
            (sample, sample_size)
        };

        // encode sample
        bar.set_message("encoding,");
        let b = Instant::now();
        let (encoded_sample, mut output) = svtav1::encode_ivf(SvtArgs {
            input: &sample,
            ..svt_args.clone()
        })?;
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
        let mut vmaf = vmaf::run(&sample, &encoded_sample, &vmaf.ffmpeg_lavfi())?;
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
            style!(
                "- Sample {sample_n} ({:.0}%) vmaf {vmaf_score:.2}",
                100.0 * encoded_size as f32 / sample_size as f32
            )
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

    let input_size = fs::metadata(input).await?.len();
    let predicted_size = results.encoded_percent_size() * input_size as f64 / 100.0;
    let output = Output {
        vmaf: results.mean_vmaf(),
        predicted_encode_size: predicted_size as _,
        predicted_encode_percent: results.encoded_percent_size(),
        predicted_encode_time: results.estimate_encode_time(duration),
    };

    if !bar.is_hidden() {
        // encode how-to hint + predictions
        eprintln!(
            "\n{} {}\n",
            style("Encode with:").dim(),
            style!(
                "ab-av1 encode -i {input:?} --crf {crf} --preset {}",
                svt.preset
            )
            .dim()
            .italic()
        );
        // stdout result
        stdout_format.print_result(
            output.vmaf,
            output.predicted_encode_size,
            output.predicted_encode_percent,
            output.predicted_encode_time,
        );
    }

    Ok(output)
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
        if self.is_empty() {
            return 100.0;
        }
        let encoded = self.iter().map(|r| r.encoded_size).sum::<u64>() as f64;
        let sample = self.iter().map(|r| r.sample_size).sum::<u64>() as f64;
        encoded * 100.0 / sample
    }

    fn mean_vmaf(&self) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        self.iter().map(|r| r.vmaf_score).sum::<f32>() / self.len() as f32
    }

    fn estimate_encode_time(&self, input_duration: Duration) -> Duration {
        if self.is_empty() {
            return Duration::ZERO;
        }
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
                    v if v < 80.0 => style!("{}%", v).bold().green(),
                    v if v >= 100.0 => style!("{}%", v).bold().red(),
                    v => style!("{}%", v).bold(),
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

#[derive(Debug, Clone)]
pub struct Output {
    pub vmaf: f32,
    pub predicted_encode_size: u64,
    pub predicted_encode_percent: f64,
    pub predicted_encode_time: Duration,
}
