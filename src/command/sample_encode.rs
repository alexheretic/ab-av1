mod cache;

use crate::{
    command::{
        args::{self, PixelFormat},
        sample_encode::cache::ScoringInfo,
        SmallDuration, PROGRESS_CHARS,
    },
    console_ext::style,
    ffmpeg::{self, FfmpegEncodeArgs},
    ffprobe::{self, Ffprobe},
    log::ProgressLogger,
    process::FfmpegOut,
    sample, temporary,
    vmaf::{self, VmafOut},
    xpsnr::{self, XpsnrOut},
};
use anyhow::{ensure, Context};
use clap::{ArgAction, Parser};
use console::style;
use futures_util::Stream;
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use log::info;
use std::{
    fmt::Display,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    pin::pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::fs;
use tokio_stream::StreamExt;

/// Encode & analyse input samples to predict how a full encode would go.
/// This is much quicker than a full encode/vmaf run.
///
/// Outputs:
/// * Mean sample score
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

    #[clap(flatten)]
    pub score: args::ScoreArgs,

    /// Calculate a XPSNR score instead of VMAF.
    #[arg(long)]
    pub xpsnr: bool,
}

pub async fn sample_encode(mut args: Args) -> anyhow::Result<()> {
    const BAR_LEN: u64 = 1024 * 1024 * 1024;
    const BAR_LEN_F: f32 = BAR_LEN as _;

    let bar = ProgressBar::new(BAR_LEN).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(Duration::from_millis(100));

    let probe = ffprobe::probe(&args.args.input);
    args.sample
        .set_extension_from_input(&args.args.input, &args.args.encoder, &probe);

    let enc_args = args.args.clone();
    let crf = args.crf;
    let stdout_fmt = args.stdout_format;
    let input_is_image = probe.is_image;

    let mut run = pin!(run(args, probe.into()));
    while let Some(update) = run.next().await {
        match update? {
            Update::Status(Status {
                work,
                fps,
                progress,
                sample,
                samples,
                full_pass,
            }) => {
                match full_pass {
                    true => bar.set_prefix("Full pass"),
                    false => bar.set_prefix(format!("Sample {sample}/{samples}")),
                }
                let label = work.fps_label();
                match work {
                    Work::Encode if fps <= 0.0 => bar.set_message("encoding,  "),
                    _ if fps <= 0.0 => bar.set_message(format!("{label},       ")),
                    _ => bar.set_message(format!("{label} {fps} fps, ")),
                }
                bar.set_position((progress * BAR_LEN_F).round() as _);
            }
            Update::SampleResult { sample, result } => result.print_attempt(&bar, sample, None),
            Update::Done(output) => {
                bar.finish();
                if io::stderr().is_terminal() {
                    eprintln!(
                        "\n{} {}\n",
                        style("Encode with:").dim(),
                        style(enc_args.encode_hint(crf)).dim().italic(),
                    );
                }
                stdout_fmt.print_result(&output, input_is_image);
            }
        }
    }
    Ok(())
}

pub fn run(
    Args {
        args,
        crf,
        sample: sample_args,
        cache,
        stdout_format: _,
        vmaf,
        score,
        xpsnr,
    }: Args,
    input_probe: Arc<Ffprobe>,
) -> impl Stream<Item = anyhow::Result<Update>> {
    async_stream::try_stream! {
        let input = Arc::new(args.input.clone());
        let input_pixel_format = input_probe.pixel_format();
        let input_is_image = input_probe.is_image;
        let input_len = fs::metadata(&*input).await?.len();
        let enc_args = args.to_encoder_args(crf, &input_probe)?;
        let duration = input_probe.duration.clone()?;
        let input_fps = input_probe.fps.clone()?;
        let samples = sample_args.sample_count(duration).max(1);
        let keep = sample_args.keep;
        let temp_dir = sample_args.temp_dir;
        let scoring = match xpsnr {
            true => ScoringInfo::Xpsnr(&score),
            _ => ScoringInfo::Vmaf(&vmaf, &score),
        };

        let (samples, sample_duration, full_pass) = {
            if input_is_image {
                (1, duration.max(Duration::from_secs(1)), true)
            } else if sample_args.sample_duration.is_zero()
                || sample_args.sample_duration * samples as _ >= duration.mul_f64(0.85)
            {
                // if the sample time is most of the full input time just encode the whole thing
                (1, duration, true)
            } else {
                let sample_duration = if input_fps > 0.0 {
                    // if sample-length is lower than a single frame use the frame time
                    let one_frame_duration = Duration::from_secs_f64(1.0 / input_fps);
                    sample_args.sample_duration.max(one_frame_duration)
                } else {
                    sample_args.sample_duration
                };
                (samples, sample_duration, false)
            }
        };
        let sample_duration_us = sample_duration.as_micros_u64();

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
                        sample_duration,
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
            let (sample_idx, sample) = match sample_tasks.recv().await {
                Some(s) => s,
                None => break,
            };
            let sample_n = sample_idx + 1;
            let (sample, sample_size) = sample?;

            info!("encoding sample {sample_n}/{samples} crf {crf}");
            yield Update::Status(Status {
                work: Work::Encode,
                fps: 0.0,
                progress: sample_idx as f32 / samples as f32,
                full_pass,
                sample: sample_n,
                samples,
            });

            // encode sample
            let result = match cache::cached_encode(
                cache,
                &sample,
                duration,
                input.extension(),
                input_len,
                full_pass,
                &enc_args,
                scoring,
            )
            .await
            {
                (Some(result), _) => {
                    if samples > 1 {
                        result.log_attempt(sample_n, samples, crf);
                    }
                    result
                }
                (None, key) => {
                    let b = Instant::now();
                    let mut logger = ProgressLogger::new(module_path!(), b);
                    let (encoded_sample, mut output) = ffmpeg::encode_sample(
                        FfmpegEncodeArgs {
                            input: &sample,
                            ..enc_args.clone()
                        },
                        temp_dir.clone(),
                        sample_args.extension.as_deref().unwrap_or("mkv"),
                    )?;
                    while let Some(enc_progress) = output.next().await {
                        if let FfmpegOut::Progress { time, fps, .. } = enc_progress? {
                            yield Update::Status(Status {
                                work: Work::Encode,
                                fps,
                                progress: (time.as_micros_u64() + sample_idx * sample_duration_us * 2) as f32
                                    / (sample_duration_us * samples * 2) as f32,
                                full_pass,
                                sample: sample_n,
                                samples,
                            });
                            logger.update(sample_duration, time, fps);
                        }
                    }
                    let encode_time = b.elapsed();
                    let encoded_size = fs::metadata(&encoded_sample).await?.len();
                    let encoded_probe = ffprobe::probe(&encoded_sample);

                    let result = match scoring {
                        ScoringInfo::Vmaf(..) => {
                            yield Update::Status(Status {
                                work: Work::Score(ScoreKind::Vmaf),
                                fps: 0.0,
                                progress: (sample_idx as f32 + 0.5) / samples as f32,
                                full_pass,
                                sample: sample_n,
                                samples,
                            });
                            let vmaf = vmaf::run(
                                &sample,
                                &encoded_sample,
                                &vmaf.ffmpeg_lavfi(
                                    encoded_probe.resolution,
                                    enc_args
                                        .pix_fmt
                                        .max(input_pixel_format.unwrap_or(PixelFormat::Yuv444p10le)),
                                    score.reference_vfilter.as_deref().or(args.vfilter.as_deref()),
                                ),
                                vmaf.fps(),
                            )?;
                            let mut vmaf = pin!(vmaf);
                            let mut logger = ProgressLogger::new("ab_av1::vmaf", Instant::now());
                            let mut vmaf_score = None;
                            while let Some(vmaf) = vmaf.next().await {
                                match vmaf {
                                    VmafOut::Done(score) => {
                                        vmaf_score = Some(score);
                                        break;
                                    }
                                    VmafOut::Progress(FfmpegOut::Progress { time, fps, .. }) => {
                                        yield Update::Status(Status {
                                            work: Work::Score(ScoreKind::Vmaf),
                                            fps,
                                            progress: (sample_duration_us +
                                                time.as_micros_u64() +
                                                sample_idx * sample_duration_us * 2) as f32
                                                / (sample_duration_us * samples * 2) as f32,
                                            full_pass,
                                            sample: sample_n,
                                            samples,
                                        });
                                        logger.update(sample_duration, time, fps);
                                    }
                                    VmafOut::Progress(_) => {}
                                    VmafOut::Err(e) => Err(e)?,
                                }
                            }

                            EncodeResult {
                                score: vmaf_score.context("no vmaf score")?,
                                score_kind: ScoreKind::Vmaf,
                                sample_size,
                                encoded_size,
                                encode_time,
                                sample_duration: encoded_probe
                                    .duration
                                    .ok()
                                    .filter(|d| !d.is_zero())
                                    .unwrap_or(sample_duration),
                                from_cache: false,
                            }
                        }
                        ScoringInfo::Xpsnr(..) => {
                            yield Update::Status(Status {
                                work: Work::Score(ScoreKind::Xpsnr),
                                fps: 0.0,
                                progress: (sample_idx as f32 + 0.5) / samples as f32,
                                full_pass,
                                sample: sample_n,
                                samples,
                            });

                            let lavfi = super::xpsnr::lavfi(
                                score.reference_vfilter.as_deref().or(args.vfilter.as_deref())
                            );
                            let xpsnr_out = xpsnr::run(&sample, &encoded_sample, &lavfi)?;
                            let mut xpsnr_out = pin!(xpsnr_out);
                            let mut logger = ProgressLogger::new("ab_av1::xpsnr", Instant::now());
                            let mut score = None;
                            while let Some(next) = xpsnr_out.next().await {
                                match next {
                                    XpsnrOut::Done(s) => {
                                        score = Some(s);
                                        break;
                                    }
                                    XpsnrOut::Progress(FfmpegOut::Progress { time, fps, .. }) => {
                                        yield Update::Status(Status {
                                            work: Work::Score(ScoreKind::Xpsnr),
                                            fps,
                                            progress: (sample_duration_us +
                                                time.as_micros_u64() +
                                                sample_idx * sample_duration_us * 2) as f32
                                                / (sample_duration_us * samples * 2) as f32,
                                            full_pass,
                                            sample: sample_n,
                                            samples,
                                        });
                                        logger.update(sample_duration, time, fps);
                                    }
                                    XpsnrOut::Progress(_) => {}
                                    XpsnrOut::Err(e) => Err(e)?,
                                }
                            }

                            EncodeResult {
                                score: score.context("no xpsnr score")?,
                                score_kind: ScoreKind::Xpsnr,
                                sample_size,
                                encoded_size,
                                encode_time,
                                sample_duration: encoded_probe
                                    .duration
                                    .ok()
                                    .filter(|d| !d.is_zero())
                                    .unwrap_or(sample_duration),
                                from_cache: false,
                            }
                        }
                    };

                    if samples > 1 {
                        result.log_attempt(sample_n, samples, crf);
                    }

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

            results.push(result.clone());
            yield Update::SampleResult { sample: sample_n, result };
        }

        let score_kind = results.score_kind();
        let output = Output {
            score: results.mean_score(),
            score_kind,
            // Using file size * encode_percent can over-estimate. However, if it ends up less
            // than the duration estimation it may turn out to be more accurate.
            predicted_encode_size: results
                .estimate_encode_size_by_duration(duration, full_pass)
                .min(estimate_encode_size_by_file_percent(&results, &input, full_pass).await?),
            encode_percent: results.encoded_percent_size(),
            predicted_encode_time: results.estimate_encode_time(duration, full_pass),
            from_cache: results.iter().all(|r| r.from_cache),
        };
        info!(
            "crf {crf} {score_kind} {:.2} predicted video stream size {} ({:.0}%) taking {}{}",
            output.score,
            HumanBytes(output.predicted_encode_size),
            output.encode_percent,
            HumanDuration(output.predicted_encode_time),
            if output.from_cache { " (cache)" } else { "" }
        );

        yield Update::Done(output);
    }
}

/// Copy a sample from the input to the temp_dir (or input dir).
async fn sample(
    input: Arc<PathBuf>,
    sample_idx: u64,
    samples: u64,
    sample_duration: Duration,
    duration: Duration,
    fps: f64,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<(Arc<PathBuf>, u64)> {
    let sample_n = sample_idx + 1;

    let sample_start = (duration.saturating_sub(sample_duration * samples as _)
        / (samples as u32 + 1))
        * sample_n as _
        + sample_duration * sample_idx as _;

    let sample_frames = ((sample_duration.as_secs_f64() * fps).round() as u32).max(1);
    let floor_to_sec = sample_duration >= Duration::from_secs(2);

    let sample = sample::copy(&input, sample_start, floor_to_sec, sample_frames, temp_dir).await?;
    let sample_size = fs::metadata(&sample).await?.len();
    ensure!(
        // ffmpeg copy may fail successfully and give us a small/empty output
        sample_size > 1024,
        "ffmpeg copy failed: encoded sample too small"
    );
    Ok((sample.into(), sample_size))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncodeResult {
    pub sample_size: u64,
    pub encoded_size: u64,
    pub score: f32,
    pub score_kind: ScoreKind,
    pub encode_time: Duration,
    /// Duration of the sample.
    ///
    /// This should be close to `SAMPLE_SIZE` but may deviate due to how samples are cut.
    pub sample_duration: Duration,
    /// Result read from cache.
    pub from_cache: bool,
}

impl EncodeResult {
    pub fn print_attempt(&self, bar: &ProgressBar, sample_n: u64, crf: Option<f32>) {
        let Self {
            sample_size,
            encoded_size,
            score,
            score_kind,
            from_cache,
            ..
        } = self;
        bar.println(
            style!(
                "- {}Sample {sample_n} ({:.0}%) {score_kind} {score:.2}{}",
                crf.map(|crf| format!("crf {crf}: ")).unwrap_or_default(),
                100.0 * *encoded_size as f32 / *sample_size as f32,
                if *from_cache { " (cache)" } else { "" },
            )
            .dim()
            .to_string(),
        );
    }

    pub fn log_attempt(&self, sample_n: u64, samples: u64, crf: f32) {
        let Self {
            sample_size,
            encoded_size,
            score,
            score_kind,
            from_cache,
            ..
        } = self;
        info!(
            "sample {sample_n}/{samples} crf {crf} {score_kind} {score:.2} ({:.0}%){}",
            100.0 * *encoded_size as f32 / *sample_size as f32,
            if *from_cache { " (cache)" } else { "" }
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScoreKind {
    Vmaf,
    Xpsnr,
}

impl ScoreKind {
    /// Display label for fps in progress bar.
    pub fn fps_label(&self) -> &'static str {
        match self {
            Self::Vmaf => "vmaf",
            Self::Xpsnr => "xpsnr",
        }
    }

    /// General display name.
    pub fn display_str(&self) -> &'static str {
        match self {
            Self::Vmaf => "VMAF",
            Self::Xpsnr => "XPSNR",
        }
    }
}

impl Display for ScoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_str())
    }
}

trait EncodeResults {
    fn encoded_percent_size(&self) -> f64;

    fn score_kind(&self) -> ScoreKind;

    fn mean_score(&self) -> f32;

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

    fn score_kind(&self) -> ScoreKind {
        self.first()
            .map(|r| r.score_kind)
            .unwrap_or(ScoreKind::Vmaf)
    }

    fn mean_score(&self) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        self.iter().map(|r| r.score).sum::<f32>() / self.len() as f32
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
    fn print_result(
        self,
        Output {
            score,
            score_kind,
            predicted_encode_size,
            encode_percent,
            predicted_encode_time,
            from_cache: _,
        }: &Output,
        image: bool,
    ) {
        match self {
            Self::Human => {
                let score = match (*score, score_kind) {
                    (v, ScoreKind::Vmaf) if v >= 95.0 => style(v).bold().green(),
                    (v, ScoreKind::Vmaf) if v < 80.0 => style(v).bold().red(),
                    (v, _) => style(v).bold(),
                };
                let percent = encode_percent.round();
                let size = match *predicted_encode_size {
                    v if percent < 80.0 => style(HumanBytes(v)).bold().green(),
                    v if percent >= 100.0 => style(HumanBytes(v)).bold().red(),
                    v => style(HumanBytes(v)).bold(),
                };
                let percent = match percent {
                    v if v < 80.0 => style!("{}%", v).bold().green(),
                    v if v >= 100.0 => style!("{}%", v).bold().red(),
                    v => style!("{}%", v).bold(),
                };
                let time = style(HumanDuration(*predicted_encode_time)).bold();
                let enc_description = match image {
                    true => "image",
                    false => "video stream",
                };
                println!(
                    "{score_kind} {score:.2} predicted {enc_description} size {size} ({percent}) taking {time}"
                );
            }
            Self::Json => {
                let mut json = serde_json::json!({
                    "predicted_encode_size": predicted_encode_size,
                    "predicted_encode_percent": encode_percent,
                    "predicted_encode_seconds": predicted_encode_time.as_secs(),
                });
                match score_kind {
                    ScoreKind::Vmaf => json["vmaf"] = (*score).into(),
                    ScoreKind::Xpsnr => json["xpsnr"] = (*score).into(),
                }
                println!("{json}");
            }
        }
    }
}

/// Sample encode result.
#[derive(Debug, Clone)]
pub struct Output {
    /// Sample mean score.
    pub score: f32,
    pub score_kind: ScoreKind,
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

/// Kinds of sample-encode work.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    #[default]
    Encode,
    Score(ScoreKind),
}

impl Work {
    /// Display label for fps in progress bar.
    pub fn fps_label(&self) -> &'static str {
        match self {
            Self::Encode => "enc",
            Self::Score(kind) => kind.fps_label(),
        }
    }
}

#[derive(Debug)]
pub struct Status {
    /// Kind of work being performed
    pub work: Work,
    /// fps, `0.0` may be interpreted as "unknown"
    pub fps: f32,
    /// sample progress `[0, 1]`
    pub progress: f32,
    /// Sample number `1,....,n`
    pub sample: u64,
    /// Total samples
    pub samples: u64,
    /// Encoding the entire input video
    pub full_pass: bool,
}

#[derive(Debug)]
pub enum Update {
    Status(Status),
    SampleResult {
        /// Sample number `1,....,n`
        sample: u64,
        result: EncodeResult,
    },
    Done(Output),
}
