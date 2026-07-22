mod err;

pub use err::Error;

use crate::{
    command::{
        PROGRESS_CHARS, args,
        sample_encode::{self, StdoutFormat, Work},
    },
    console_ext::style,
    ffprobe::{self, Ffprobe},
    float::TerseF32,
};
use anyhow::Context;
use clap::{ArgAction, Parser};
use console::style;
use futures_util::{Stream, StreamExt};
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use log::info;
use std::{io::IsTerminal, pin::pin, sync::Arc, time::Duration};

const BAR_LEN: u64 = 1024 * 1024 * 1024;
const DEFAULT_MIN_VMAF: f32 = 95.0;

/// Interpolated binary search using sample-encode to find the best crf
/// value delivering min-vmaf & max-encoded-percent.
///
/// Outputs:
/// * Best crf value
/// * Mean sample VMAF score
/// * Predicted full encode size
/// * Predicted full encode time
///
/// Use -v to print per-sample results.
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
#[group(skip)]
pub struct Args {
    #[clap(flatten)]
    pub search: SearchArgs,

    /// Stdout message format `human` or `json`.
    ///
    /// See <https://github.com/alexheretic/ab-av1/blob/main/stdout-format-json.md>
    #[arg(long, value_enum, default_value_t = StdoutFormat::Human)]
    pub stdout_format: StdoutFormat,
}

/// Search args shared with auto-encode.
#[derive(Parser)]
#[group(skip)]
pub struct SearchArgs {
    #[clap(flatten)]
    pub args: args::Encode,

    /// Desired min VMAF score to deliver.
    ///
    /// [default: 95]
    #[arg(long, group = "min_score")]
    pub min_vmaf: Option<f32>,

    /// Desired min XPSNR score to deliver.
    ///
    /// Enables use of XPSNR for score analysis instead of VMAF.
    #[arg(long, group = "min_score")]
    pub min_xpsnr: Option<f32>,

    /// Maximum desired encoded size percentage of the input size.
    #[arg(long, default_value_t = 80.0)]
    pub max_encoded_percent: f32,

    /// Minimum (highest quality) crf value to try.
    ///
    /// [default: 10, 5 for svt-av1, 2 for mpeg2video]
    #[arg(long)]
    pub min_crf: Option<f32>,

    /// Maximum (lowest quality) crf value to try.
    ///
    /// [default: 55, 46 for x264,x265, 255 for rav1e,av1_vaapi, 30 for mpeg2video]
    #[arg(long)]
    pub max_crf: Option<f32>,

    /// Keep searching until a crf is found no more than min_vmaf+0.05 or all
    /// possibilities have been attempted.
    ///
    /// By default the "higher vmaf tolerance" increases with each attempt (0.1, 0.2, 0.4 etc...).
    #[arg(long)]
    pub thorough: bool,

    /// Constant rate factor search increment precision.
    ///
    /// [default: 1.0, 0.1 for x264,x265,vp9]
    #[arg(long)]
    pub crf_increment: Option<f32>,

    /// Set the interpretation of crf so that higher crfs mean higher quality.
    /// For most encoders *lower* crfs mean higher quality.
    ///
    /// [default: false, true for hevc_videotoolbox]
    #[arg(long, num_args=0..=1, default_missing_value = "true")]
    pub high_crf_means_hq: Option<bool>,

    /// Enable sample-encode caching.
    #[arg(
        long,
        default_value_t = true,
        env = "AB_AV1_CACHE",
        action(ArgAction::Set)
    )]
    pub cache: bool,

    #[clap(flatten)]
    pub sample: args::Sample,

    #[clap(flatten)]
    pub vmaf: args::Vmaf,

    #[clap(flatten)]
    pub score: args::ScoreArgs,

    #[clap(flatten)]
    pub xpsnr: args::Xpsnr,

    #[command(flatten)]
    pub verbose: clap_verbosity_flag::Verbosity,
}

impl SearchArgs {
    pub fn min_score(&self) -> f32 {
        self.min_vmaf.or(self.min_xpsnr).unwrap_or(DEFAULT_MIN_VMAF)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.min_vmaf.is_none()
            && let Some(num) = self
                .vmaf
                .vmaf_args
                .iter()
                .find_map(|arg| arg.parse::<f32>().ok())
        {
            anyhow::bail!("Invalid use of --vmaf NUMBER, did you mean: --min-vmaf {num}");
        }
        Ok(())
    }
}

pub async fn crf_search(
    Args {
        mut search,
        stdout_format,
    }: Args,
) -> anyhow::Result<()> {
    search.validate()?;

    let bar = ProgressBar::new(BAR_LEN).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(Duration::from_millis(100));

    let probe = ffprobe::probe(&search.args.input);
    let input_is_image = probe.is_image;
    search
        .sample
        .set_extension_from_input(&search.args.input, &search.args.encoder, &probe);

    let min_score = search.min_score();
    let max_encoded_percent = search.max_encoded_percent;
    let thorough = search.thorough;
    let enc_args = search.args.clone();
    let verbose = search.verbose;

    let mut run = pin!(run(search, probe.into()));
    while let Some(update) = run.next().await {
        let update = update.inspect_err(|e| {
            if let Error::NoGoodCrf { last } = e {
                last.print_attempt(&bar, min_score, max_encoded_percent);
                if let StdoutFormat::Json = stdout_format {
                    println!("{}", last.attempt_json());
                    println!("{}", error_json(e));
                }
            }
        })?;
        match update {
            Update::Status {
                crf_run,
                crf,
                sample:
                    sample_encode::Status {
                        work,
                        fps,
                        progress,
                        sample,
                        samples,
                        full_pass,
                    },
            } => {
                bar.set_position(guess_progress(crf_run, progress, thorough) as _);
                let crf = TerseF32(crf);
                match full_pass {
                    true => bar.set_prefix(format!("crf {crf} full pass")),
                    false => bar.set_prefix(format!("crf {crf} {sample}/{samples}")),
                }
                let label = work.fps_label();
                match work {
                    Work::Encode if fps <= 0.0 => bar.set_message("encoding,  "),
                    _ if fps <= 0.0 => bar.set_message(format!("{label},       ")),
                    _ => bar.set_message(format!("{label} {fps} fps, ")),
                }
            }
            Update::SampleResult {
                crf,
                sample,
                result,
            } => {
                if verbose
                    .log_level()
                    .is_some_and(|lvl| lvl > log::Level::Error)
                {
                    result.print_attempt(&bar, sample, Some(crf))
                }
            }
            Update::SampleEncodeDone(sample) => {
                if let StdoutFormat::Json = stdout_format {
                    println!("{}", sample.enc.sample_encode_done_json(sample.crf));
                }
            }
            Update::RunResult(result) => {
                result.print_attempt(&bar, min_score, max_encoded_percent);
                if let StdoutFormat::Json = stdout_format {
                    println!("{}", result.attempt_json());
                }
            }
            Update::Done(best) => {
                info!("crf {} successful", best.crf);
                bar.finish_with_message("");
                if std::io::stderr().is_terminal() {
                    eprintln!(
                        "\n{} {}\n",
                        style("Encode with:").dim(),
                        style(enc_args.encode_hint(best.crf)).dim().italic(),
                    );
                }
                match stdout_format {
                    StdoutFormat::Human => best.print_result_human(input_is_image),
                    StdoutFormat::Json => println!("{}", best.done_json()),
                }
                return Ok(());
            }
        }
    }
    unreachable!()
}

pub fn run(
    SearchArgs {
        args,
        min_vmaf,
        min_xpsnr,
        max_encoded_percent,
        min_crf,
        max_crf,
        crf_increment,
        high_crf_means_hq,
        thorough,
        sample,
        cache,
        vmaf,
        score,
        xpsnr,
        verbose: _,
    }: SearchArgs,
    input_probe: Arc<Ffprobe>,
) -> impl Stream<Item = Result<Update, Error>> {
    async_stream::try_stream! {
        let default_max_crf = args.encoder.default_max_crf();
        let max_crf = max_crf.unwrap_or(default_max_crf);
        let default_min_crf = args.encoder.default_min_crf();
        let min_crf = min_crf.unwrap_or(default_min_crf);
        Error::ensure_other(min_crf < max_crf, "Invalid --min-crf & --max-crf")?;
        // by default use vmaf 95, otherwise use whatever is specified
        let min_score = min_vmaf.or(min_xpsnr).unwrap_or(DEFAULT_MIN_VMAF);

        // Whether to make the 2nd iteration on the ~20%/~80% crf point instead of the min/max to
        // improve interpolation by narrowing the crf range a 20% (or 30%) subrange.
        //
        // 20/80% is preferred to 25/75% to account for searches in the "middle" benefitting from
        // having both bounds computed after the 2nd iteration, whereas the two edges must compute
        // the min/max crf on the 3rd iter.
        //
        // If a custom crf range is being used under half the default, this 2nd cut is not needed.
        let cut_on_iter2 = (max_crf - min_crf) > (default_max_crf - default_min_crf) * 0.5;

        let crf_increment = crf_increment
            .unwrap_or_else(|| args.encoder.default_crf_increment())
            .max(0.001);

        let q_conv = QualityConverter {
            crf_increment,
            high_crf_means_hq: high_crf_means_hq.unwrap_or_else(|| args.encoder.high_crf_means_hq()),
        };

        let (min_q, max_q) = q_conv.min_max_q(min_crf, max_crf);
        assert!(min_q < max_q);
        let mut q = (min_q + max_q) / 2;

        let mut args = sample_encode::Args {
            args: args.clone(),
            crf: 0.0,
            sample: sample.clone(),
            cache,
            stdout_format: sample_encode::StdoutFormat::Json,
            vmaf: vmaf.clone(),
            score: score.clone(),
            xpsnr: min_xpsnr.is_some(),
            xpsnr_opts: xpsnr,
        };

        let mut crf_attempts = Vec::new();

        for run in 1.. {
            // how much we're prepared to go higher than the min-vmaf
            let higher_tolerance = match thorough {
                true => 0.05,
                // increment 1.0 => +0.1, +0.2, +0.4, +0.8 ..
                // increment 0.1 => +0.1, +0.1, +0.1, +0.16 ..
                _ => (crf_increment.min(1.0) * 2_f32.powi(run as i32 - 1) * 0.1).max(0.1),
            };
            args.crf = q_conv.crf(q);

            let mut sample_enc = pin!(sample_encode::run(args.clone(), input_probe.clone()));
            let mut sample_enc_output = None;
            while let Some(update) = sample_enc.next().await {
                match update? {
                    sample_encode::Update::Status(status) => {
                        yield Update::Status { crf_run: run, crf: args.crf, sample: status };
                    }
                    sample_encode::Update::SampleResult { sample, result } => {
                        yield Update::SampleResult { crf: args.crf, sample, result };
                    }
                    sample_encode::Update::Done(output) => sample_enc_output = Some(output),
                }
            }

            let sample = Sample {
                crf: args.crf,
                q,
                enc: sample_enc_output.context("no sample output?")?,
            };
            let score = sample.enc.single_score();
            crf_attempts.push(sample.clone());
            yield Update::SampleEncodeDone(sample.clone());
            let sample_small_enough = sample.enc.encode_percent <= max_encoded_percent as _;

            if score > min_score {
                // good
                if sample_small_enough && score < min_score + higher_tolerance {
                    yield Update::Done(sample);
                    return;
                }
                let u_bound = crf_attempts
                    .iter()
                    .filter(|s| s.q > sample.q)
                    .min_by_key(|s| s.q);

                match u_bound {
                    Some(upper) if upper.q == sample.q + 1 => {
                        Error::ensure_or_no_good_crf(sample_small_enough, &sample)?;
                        yield Update::Done(sample);
                        return;
                    }
                    Some(upper) => {
                        q = vmaf_lerp_q(min_score, upper, &sample);
                    }
                    None if sample.q == max_q => {
                        Error::ensure_or_no_good_crf(sample_small_enough, &sample)?;
                        yield Update::Done(sample);
                        return;
                    }
                    None if cut_on_iter2 && run == 1 && sample.q + 1 < max_q => {
                        q = (sample.q as f32 * 0.4 + max_q as f32 * 0.6).round() as _;
                    }
                    None => q = max_q,
                };
            } else {
                // not good enough
                if !sample_small_enough || sample.q == min_q {
                    Err(Error::NoGoodCrf { last: sample.clone() })?;
                }

                let l_bound = crf_attempts
                    .iter()
                    .filter(|s| s.q < sample.q)
                    .max_by_key(|s| s.q);

                match l_bound {
                    Some(lower) if lower.q + 1 == sample.q => {
                        Error::ensure_or_no_good_crf(lower.enc.encode_percent <= max_encoded_percent as _, &sample)?;
                        yield Update::RunResult(sample.clone());
                        yield Update::Done(lower.clone());
                        return;
                    }
                    Some(lower) => {
                        q = vmaf_lerp_q(min_score, &sample, lower);
                    }
                    None if cut_on_iter2 && run == 1 && sample.q > min_q + 1 => {
                        q = (sample.q as f32 * 0.4 + min_q as f32 * 0.6).round() as _;
                    }
                    None => q = min_q,
                };
            }
            yield Update::RunResult(sample.clone());
        }
        unreachable!();
    }
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub enc: sample_encode::Output,
    pub crf: f32,
    q: i64,
}

impl Sample {
    pub fn print_attempt(&self, bar: &ProgressBar, min_score: f32, max_encoded_percent: f32) {
        if bar.is_hidden() {
            info!(
                "crf {} {} {:.2} ({:.0}%){}",
                TerseF32(self.crf),
                self.enc.single_score_kind(),
                self.enc.single_score(),
                self.enc.encode_percent,
                if self.enc.from_cache { " (cache)" } else { "" }
            );
            return;
        }

        let crf_label = style("- crf").dim();
        let mut crf = style(TerseF32(self.crf));

        let score_v = self.enc.single_score();
        let mut score = style(score_v);
        let score_label = style(self.enc.single_score_kind()).dim();
        let mut percent = style!("{:.0}%", self.enc.encode_percent);
        let open = style("(").dim();
        let close = style(")").dim();
        let cache_msg = match self.enc.from_cache {
            true => style(" (cache)").dim(),
            false => style(""),
        };

        if score_v < min_score {
            crf = crf.red().bright();
            score = score.red().bright();
        }
        if self.enc.encode_percent > max_encoded_percent as _ {
            crf = crf.red().bright();
            percent = percent.red().bright();
        }

        bar.println(format!(
            "{crf_label} {crf} {score_label} {score:.2} {open}{percent}{close}{cache_msg}"
        ));
    }

    pub fn print_result_human(&self, image: bool) {
        let crf = style(TerseF32(self.crf)).bold().green();
        let enc = &self.enc;
        let score = style(enc.single_score()).bold().green();
        let score_kind = enc.single_score_kind();
        let size = style(HumanBytes(enc.predicted_encode_size)).bold().green();
        let percent = style!("{}%", enc.encode_percent.round()).bold().green();
        let time = style(HumanDuration(enc.predicted_encode_time)).bold();
        let enc_description = match image {
            true => "image",
            false => "video stream",
        };
        println!(
            "crf {crf} {score_kind} {score:.2} predicted {enc_description} size {size} ({percent}) taking {time}"
        );
    }

    /// `crf-search-attempt` json message, see _stdout-format-json.md_.
    pub fn attempt_json(&self) -> serde_json::Value {
        let mut json = serde_json::json!({
            "type": "crf-search-attempt",
            "crf": self.crf,
            "from_cache": self.enc.from_cache,
            "predicted_encode_percent": self.enc.encode_percent,
        });
        if let Some(score) = self.enc.vmaf_score {
            json["vmaf"] = score.into();
        }
        if let Some(score) = self.enc.xpsnr_score {
            json["xpsnr"] = score.into();
        }
        json
    }

    /// `crf-search-done` json message, see _stdout-format-json.md_.
    pub fn done_json(&self) -> serde_json::Value {
        let mut json = self.enc.sample_encode_done_json(self.crf);
        json["type"] = "crf-search-done".into();
        json
    }
}

/// `crf-search-error` json message, see _stdout-format-json.md_.
fn error_json(err: &Error) -> serde_json::Value {
    serde_json::json!({
        "type": "crf-search-error",
        "message": err.to_string(),
    })
}

#[cfg(test)]
fn test_sample() -> Sample {
    Sample {
        enc: sample_encode::Output {
            vmaf_score: Some(95.5),
            xpsnr_score: None,
            predicted_encode_size: 38889644,
            encode_percent: 41.25,
            predicted_encode_time: Duration::from_secs(1560),
            from_cache: false,
        },
        crf: 34.0,
        q: 34,
    }
}

#[test]
fn attempt_json_message() {
    assert_eq!(
        test_sample().attempt_json().to_string(),
        r#"{"crf":34.0,"from_cache":false,"predicted_encode_percent":41.25,"type":"crf-search-attempt","vmaf":95.5}"#
    );
}

#[test]
fn done_json_message() {
    assert_eq!(
        test_sample().done_json().to_string(),
        r#"{"crf":34.0,"from_cache":false,"predicted_encode_percent":41.25,"predicted_encode_seconds":1560.0,"predicted_encode_size":38889644,"type":"crf-search-done","vmaf":95.5}"#
    );
}

#[test]
fn error_json_message() {
    let err = Error::NoGoodCrf {
        last: test_sample(),
    };
    assert_eq!(
        error_json(&err).to_string(),
        r#"{"message":"Failed to find a suitable crf","type":"crf-search-error"}"#
    );
}

#[test]
fn parse_stdout_format() {
    Args::try_parse_from(["crf-search", "-i", "vid.mkv", "--stdout-format", "json"])
        .expect("--stdout-format json should parse");
}

/// Produce a q value between given samples using vmaf score linear interpolation
/// so the output q value should produce the `min_vmaf`.
///
/// Note: `worse_q` will be a numerically higher q value (worse quality),
///       `better_q` a numerically lower q value (better quality).
///
/// # Issues
/// Crf values do not linearly map to VMAF changes (or anything?) so this is a flawed method,
/// though it seems to work better than a binary search.
/// Perhaps a better approximation of a general crf->vmaf model could be found.
/// This would be helpful particularly for small crf-increments.
fn vmaf_lerp_q(min_vmaf: f32, worse_q: &Sample, better_q: &Sample) -> i64 {
    assert!(
        worse_q.enc.single_score() <= min_vmaf
            && worse_q.enc.single_score() < better_q.enc.single_score()
            && worse_q.q > better_q.q,
        "invalid vmaf_lerp_crf usage: ({min_vmaf}, {worse_q:?}, {better_q:?})"
    );

    let vmaf_diff = better_q.enc.single_score() - worse_q.enc.single_score();
    let vmaf_factor = (min_vmaf - worse_q.enc.single_score()) / vmaf_diff;

    let q_diff = worse_q.q - better_q.q;
    let lerp = (worse_q.q as f32 - q_diff as f32 * vmaf_factor).round() as i64;
    lerp.clamp(better_q.q + 1, worse_q.q - 1)
}

/// sample_progress: [0, 1]
pub fn guess_progress(run: usize, sample_progress: f32, thorough: bool) -> f64 {
    let total_runs_guess = match () {
        // Guess 6 iterations for a "thorough" search
        _ if thorough && run < 7 => 6.0,
        // Guess 4 iterations initially
        _ if run < 5 => 4.0,
        // Otherwise guess next will work
        _ => run as f64,
    };
    ((run - 1) as f64 + sample_progress as f64) * BAR_LEN as f64 / total_runs_guess
}

/// Conversion logic for integer "q" values used in the crf search.
///
/// "q" values are
/// * integers
/// * low q means higher quality
/// * they can be converted to/from crf
struct QualityConverter {
    high_crf_means_hq: bool,
    crf_increment: f32,
}

impl QualityConverter {
    /// Calculate "q" as an integer quality value related to crf.
    ///
    /// # Example
    /// * crf=33.5, inc=0.1 -> q=335
    /// * crf=27, inc=1 -> q=27
    ///
    /// # Example: high_crf_means_hq encoders
    /// * crf=33.5, inc=0.1 -> q=-335
    /// * crf=27, inc=1 -> q=-27
    pub fn q(&self, crf: f32) -> i64 {
        let q = (f64::from(crf) / f64::from(self.crf_increment)).round() as i64;
        match self.high_crf_means_hq {
            true => -q,
            false => q,
        }
    }

    /// Calculate crf back from "q".
    pub fn crf(&self, q: i64) -> f32 {
        let pos_q = match self.high_crf_means_hq {
            true => -q,
            false => q,
        };
        ((pos_q as f64) * f64::from(self.crf_increment)) as _
    }

    pub fn min_max_q(&self, min_crf: f32, max_crf: f32) -> (i64, i64) {
        match self.high_crf_means_hq {
            true => (self.q(max_crf), self.q(min_crf)),
            false => (self.q(min_crf), self.q(max_crf)),
        }
    }
}

#[test]
fn q_crf_conversions() {
    let mut q_conv = QualityConverter {
        crf_increment: 0.1,
        high_crf_means_hq: false,
    };

    assert_eq!(q_conv.q(33.5), 335);
    assert_eq!(q_conv.crf(335), 33.5);

    q_conv.crf_increment = 1.0;
    assert_eq!(q_conv.q(27.0), 27);
    assert_eq!(q_conv.crf(27), 27.0);
}

#[test]
fn q_crf_conversions_high_crf_means_hq() {
    let mut q_conv = QualityConverter {
        crf_increment: 0.1,
        high_crf_means_hq: true,
    };

    assert_eq!(q_conv.q(33.5), -335);
    assert_eq!(q_conv.crf(-335), 33.5);

    q_conv.crf_increment = 1.0;
    assert_eq!(q_conv.q(27.0), -27);
    assert_eq!(q_conv.crf(-27), 27.0);
}

#[derive(Debug)]
pub enum Update {
    Status {
        /// run number starting from `1`.
        crf_run: usize,
        /// crf of this run
        crf: f32,
        sample: sample_encode::Status,
    },
    SampleResult {
        crf: f32,
        /// Sample number `1,....,n`
        sample: u64,
        result: sample_encode::EncodeResult,
    },
    /// Sample encode of a crf attempt completed, emitted for every attempt.
    SampleEncodeDone(Sample),
    /// Run result (excludes successful final runs)
    RunResult(Sample),
    Done(Sample),
}
