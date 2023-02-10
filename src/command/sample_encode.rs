mod cache;

use crate::{
    command::{
        args::{self, PixelFormat},
        PROGRESS_CHARS,
    },
    console_ext::style,
    ffmpeg::{self, FfmpegEncodeArgs},
    ffprobe::{self, Ffprobe},
    process::FfmpegOut,
    sample, temporary, vmaf,
    vmaf::VmafOut,
    SAMPLE_SIZE, SAMPLE_SIZE_S,
};
use anyhow::ensure;
use clap::{ArgAction, Parser};
use console::style;
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
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
#[group(skip)]
pub struct Args {
    #[clap(flatten)]
    pub args: args::Encode,

    /// Encoder constant rate factor (1-63). Lower means better quality.
    #[arg(long)]
    pub crf: f32,

    #[clap(flatten)]
    pub sample: args::Sample,

    /// Keep temporary files after exiting.
    #[arg(long)]
    pub keep: bool,

    /// Enable sample-encode caching.
    #[arg(
        long,
        default_value_t = true,
        env = "AB_AV1_CACHE",
        action(ArgAction::Set)
    )]
    pub cache: bool,

    /// Stdout message format `human` or `json`.
    #[arg(long, value_enum, default_value_t = StdoutFormat::Human)]
    pub stdout_format: StdoutFormat,

    #[clap(flatten)]
    pub vmaf: args::Vmaf,
}

pub async fn sample_encode(mut args: Args) -> anyhow::Result<()> {
    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg:13} eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(Duration::from_millis(100));

    let probe = ffprobe::probe(&args.args.input);
    args.sample
        .set_extension_from_input(&args.args.input, &probe);
    run(args, probe.into(), bar).await?;
    Ok(())
}

pub async fn run(
    Args {
        args,
        crf,
        sample: sample_args,
        keep,
        cache,
        stdout_format,
        vmaf,
    }: Args,
    input_probe: Arc<Ffprobe>,
    bar: ProgressBar,
) -> anyhow::Result<Output> {
    let input = Arc::new(args.input.clone());
    let input_pixel_format = input_probe.pixel_format();
    let input_is_image = input_probe.is_image;
    let input_len = fs::metadata(&*input).await?.len();
    let enc_args = args.to_encoder_args(crf, &input_probe)?;
    let duration = input_probe.duration.clone()?;
    let input_fps = input_probe.fps.clone()?;
    let samples = sample_args.sample_count(duration).max(1);
    let temp_dir = sample_args.temp_dir;

    let (samples, sample_duration, full_pass) = {
        if input_is_image {
            (1, duration.max(Duration::from_secs(1)), true)
        } else if SAMPLE_SIZE * samples as _ >= duration.mul_f64(0.85) {
            // if the sample time is most of the full input time just encode the whole thing
            (1, duration, true)
        } else {
            (samples, SAMPLE_SIZE, false)
        }
    };
    let sample_duration_s = sample_duration.as_secs();
    bar.set_length(sample_duration_s * samples * 2);

    // Start creating copy samples async, this is IO bound & not cpu intensive
    let (tx, mut sample_tasks) = tokio::sync::mpsc::unbounded_channel();
    let sample_temp = temp_dir.clone();
    let sample_in = input.clone();
    tokio::task::spawn_local(async move {
        if full_pass {
            // Use the entire video as a single sample
            let _ = tx.send((0, Ok((sample_in.clone(), input_len))));
        } else {
            for sample_idx in 0..samples {
                let sample = sample(
                    sample_in.clone(),
                    sample_idx,
                    samples,
                    duration,
                    input_fps,
                    sample_temp.clone(),
                )
                .await;
                if tx.send((sample_idx, sample)).is_err() {
                    break;
                }
            }
        }
    });

    let mut results = Vec::new();
    loop {
        bar.set_message("sampling,");
        let (sample_idx, sample) = match sample_tasks.recv().await {
            Some(s) => s,
            None => break,
        };
        let sample_n = sample_idx + 1;
        match full_pass {
            true => bar.set_prefix("Full pass"),
            false => bar.set_prefix(format!("Sample {sample_n}/{samples}")),
        };

        let (sample, sample_size) = sample?;

        // encode sample
        let result = match cache::cached_encode(
            cache,
            &sample,
            duration,
            input.extension(),
            input_len,
            full_pass,
            &enc_args,
        )
        .await
        {
            (Some(result), _) => {
                bar.set_position(sample_n * sample_duration_s * 2);
                bar.println(
                    style!(
                        "- Sample {sample_n} ({:.0}%) vmaf {:.2} (cache)",
                        100.0 * result.encoded_size as f32 / sample_size as f32,
                        result.vmaf_score,
                    )
                    .dim()
                    .to_string(),
                );
                result
            }
            (None, key) => {
                bar.set_message("encoding,");
                let b = Instant::now();
                let (encoded_sample, mut output) = ffmpeg::encode_sample(
                    FfmpegEncodeArgs {
                        input: &sample,
                        ..enc_args.clone()
                    },
                    temp_dir.clone(),
                    sample_args.extension.as_deref().unwrap_or("mkv"),
                )?;
                while let Some(progress) = output.next().await {
                    if let FfmpegOut::Progress { time, fps, .. } = progress? {
                        bar.set_position(time.as_secs() + sample_idx * sample_duration_s * 2);
                        if fps > 0.0 {
                            bar.set_message(format!("enc {fps} fps,"));
                        }
                    }
                }
                let encode_time = b.elapsed();
                let encoded_size = fs::metadata(&encoded_sample).await?.len();
                let encoded_probe = ffprobe::probe(&encoded_sample);

                // calculate vmaf
                bar.set_message("vmaf running,");
                let mut vmaf = vmaf::run(
                    &sample,
                    args.vfilter.as_deref(),
                    &encoded_sample,
                    &vmaf.ffmpeg_lavfi(encoded_probe.resolution),
                    enc_args
                        .pix_fmt
                        .max(input_pixel_format.unwrap_or(PixelFormat::Yuv444p10le)),
                )?;
                let mut vmaf_score = -1.0;
                while let Some(vmaf) = vmaf.next().await {
                    match vmaf {
                        VmafOut::Done(score) => {
                            vmaf_score = score;
                            break;
                        }
                        VmafOut::Progress(FfmpegOut::Progress { time, fps, .. }) => {
                            bar.set_position(
                                sample_duration_s
                                    // *24/fps adjusts for vmaf `-r 24`
                                    + (time.as_secs_f64() * (24.0 / input_fps)).round() as u64
                                    + sample_idx * sample_duration_s * 2,
                            );
                            if fps > 0.0 {
                                bar.set_message(format!("vmaf {fps} fps,"));
                            }
                        }
                        VmafOut::Progress(_) => {}
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

                let result = EncodeResult {
                    vmaf_score,
                    sample_size,
                    encoded_size,
                    encode_time,
                    sample_duration: encoded_probe
                        .duration
                        .ok()
                        .filter(|d| !d.is_zero())
                        .unwrap_or(sample_duration),
                    from_cache: false,
                };

                if let Some(k) = key {
                    cache::cache_result(k, &result).await?;
                }

                // Early clean. Note: Avoid cleaning copy samples
                temporary::clean(true).await;
                if !keep {
                    let _ = tokio::fs::remove_file(encoded_sample).await;
                }

                result
            }
        };

        results.push(result);
    }
    bar.finish();

    let output = Output {
        vmaf: results.mean_vmaf(),
        // Using file size * encode_percent can over-estimate. However, if it ends up less
        // than the duration estimation it may turn out to be more accurate.
        predicted_encode_size: results
            .estimate_encode_size_by_duration(duration, full_pass)
            .min(estimate_encode_size_by_file_percent(&results, &input, full_pass).await?),
        encode_percent: results.encoded_percent_size(),
        predicted_encode_time: results.estimate_encode_time(duration, full_pass),
        from_cache: results.iter().all(|r| r.from_cache),
    };

    if !bar.is_hidden() {
        // encode how-to hint + predictions
        eprintln!(
            "\n{} {}\n",
            style("Encode with:").dim(),
            style(args.encode_hint(crf)).dim().italic(),
        );
        // stdout result
        stdout_format.print_result(
            output.vmaf,
            output.predicted_encode_size,
            output.encode_percent,
            output.predicted_encode_time,
            input_is_image,
        );
    }

    Ok(output)
}

/// Copy a sample from the input to the temp_dir (or input dir).
async fn sample(
    input: Arc<PathBuf>,
    sample_idx: u64,
    samples: u64,
    duration: Duration,
    fps: f64,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<(Arc<PathBuf>, u64)> {
    let sample_n = sample_idx + 1;

    let sample_start =
        Duration::from_secs((duration.as_secs() - SAMPLE_SIZE_S * samples) / (samples + 1))
            * sample_n as _
            + SAMPLE_SIZE * sample_idx as _;
    let sample_frames = (SAMPLE_SIZE_S as f64 * fps).round() as u32;

    let sample = sample::copy(&input, sample_start, sample_frames, temp_dir).await?;
    let sample_size = fs::metadata(&sample).await?.len();
    ensure!(
        // ffmpeg copy may fail sucessfully and give us a small/empty output
        sample_size > 1024,
        "ffmpeg copy failed: encoded sample too small"
    );
    Ok((sample.into(), sample_size))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncodeResult {
    sample_size: u64,
    encoded_size: u64,
    vmaf_score: f32,
    encode_time: Duration,
    /// Duration of the sample.
    ///
    /// This should be close to `SAMPLE_SIZE` but may deviate due to how samples are cut.
    sample_duration: Duration,
    /// Result read from cache.
    from_cache: bool,
}

trait EncodeResults {
    fn encoded_percent_size(&self) -> f64;

    fn mean_vmaf(&self) -> f32;

    /// Return estimated encoded **video stream** size by multiplying sample size by duration.
    fn estimate_encode_size_by_duration(
        &self,
        input_duration: Duration,
        single_full_pass: bool,
    ) -> u64;

    fn estimate_encode_time(&self, input_duration: Duration, single_full_pass: bool) -> Duration;
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

    fn estimate_encode_size_by_duration(
        &self,
        input_duration: Duration,
        single_full_pass: bool,
    ) -> u64 {
        if self.is_empty() {
            return 0;
        }
        if single_full_pass {
            return self[0].encoded_size;
        }

        let sample_duration: Duration = self.iter().map(|s| s.sample_duration).sum();
        let sample_factor = input_duration.as_secs_f64() / sample_duration.as_secs_f64();
        let sample_encode_size: f64 = self.iter().map(|r| r.encoded_size as f64).sum();

        (sample_encode_size * sample_factor).round() as _
    }

    fn estimate_encode_time(&self, input_duration: Duration, single_full_pass: bool) -> Duration {
        if self.is_empty() {
            return Duration::ZERO;
        }
        if single_full_pass {
            return self[0].encode_time;
        }

        let sample_duration: Duration = self.iter().map(|s| s.sample_duration).sum();
        let sample_factor = input_duration.as_secs_f64() / sample_duration.as_secs_f64();
        let sample_encode_time: Duration = self.iter().map(|r| r.encode_time).sum();

        let estimate = sample_encode_time.mul_f64(sample_factor);
        if estimate < Duration::from_secs(1) {
            estimate
        } else {
            Duration::from_secs(estimate.as_secs())
        }
    }
}

/// Return estimated encoded **video stream** size by applying the sample percentage
/// change to the input file size.
///
/// This can over-estimate the larger the non-video proportion of the input.
async fn estimate_encode_size_by_file_percent(
    results: &Vec<EncodeResult>,
    input: &Path,
    single_full_pass: bool,
) -> anyhow::Result<u64> {
    if results.is_empty() {
        return Ok(0);
    }
    if single_full_pass {
        return Ok(results[0].encoded_size);
    }
    let encode_proportion = results.encoded_percent_size() / 100.0;

    Ok((fs::metadata(input).await?.len() as f64 * encode_proportion).round() as _)
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum StdoutFormat {
    Human,
    Json,
}

impl StdoutFormat {
    fn print_result(self, vmaf: f32, size: u64, percent: f64, time: Duration, image: bool) {
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
                let enc_description = match image {
                    true => "image",
                    false => "video stream",
                };
                println!(
                    "VMAF {vmaf:.2} predicted {enc_description} size {size} ({percent}) taking {time}"
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

/// Sample encode result.
#[derive(Debug, Clone)]
pub struct Output {
    /// Sample mean VMAF score.
    pub vmaf: f32,
    /// Estimated full encoded **video stream** size.
    ///
    /// Encoded sample size multiplied by duration.
    pub predicted_encode_size: u64,
    /// Sample mean encoded percentage.
    pub encode_percent: f64,
    /// Estimated full encode time.
    ///
    /// Sample encode time multiplied by duration.
    pub predicted_encode_time: Duration,
    /// All sample results were read from the cache.
    pub from_cache: bool,
}
