mod err;

pub use err::Error;

use crate::{
    command::{args, crf_search::err::ensure_or_no_good_crf, sample_encode, PROGRESS_CHARS},
    console_ext::style,
    ffprobe,
    ffprobe::Ffprobe,
    float::TerseF32,
};
use clap::{ArgAction, Parser};
use console::style;
use err::ensure_other;
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use std::{sync::Arc, time::Duration};

const BAR_LEN: u64 = 1000;

/// Interpolated binary search using sample-encode to find the best crf
/// value delivering min-vmaf & max-encoded-percent.
///
/// Outputs:
/// * Best crf value
/// * Mean sample VMAF score
/// * Predicted full encode size
/// * Predicted full encode time
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
#[group(skip)]
pub struct Args {
    #[clap(flatten)]
    pub args: args::Encode,

    /// Desired min VMAF score to deliver.
    #[arg(long, default_value_t = 95.0)]
    pub min_vmaf: f32,

    /// Maximum desired encoded size percentage of the input size.
    #[arg(long, default_value_t = 80.0)]
    pub max_encoded_percent: f32,

    /// Minimum (highest quality) crf value to try.
    #[arg(long, default_value_t = 10.0)]
    pub min_crf: f32,

    /// Maximum (lowest quality) crf value to try.
    ///
    /// [default: 55, 46 for x264,x265, 255 for rav1e]
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

    #[arg(skip)]
    pub quiet: bool,
}

pub async fn crf_search(mut args: Args) -> anyhow::Result<()> {
    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );

    let probe = ffprobe::probe(&args.args.input);
    let input_is_image = probe.is_image;
    args.sample
        .set_extension_from_input(&args.args.input, &probe);

    let best = run(&args, probe.into(), bar.clone()).await;
    bar.finish();
    let best = best?;

    // encode how-to hint + predictions
    eprintln!(
        "\n{} {}\n",
        style("Encode with:").dim(),
        style(args.args.encode_hint(best.crf())).dim().italic(),
    );

    StdoutFormat::Human.print_result(&best, input_is_image);

    Ok(())
}

pub async fn run(
    Args {
        args,
        min_vmaf,
        max_encoded_percent,
        min_crf,
        max_crf,
        crf_increment,
        thorough,
        sample,
        quiet,
        cache,
        vmaf,
    }: &Args,
    input_probe: Arc<Ffprobe>,
    bar: ProgressBar,
) -> Result<Sample, Error> {
    let max_crf = max_crf.unwrap_or_else(|| args.encoder.default_max_crf());
    ensure_other!(*min_crf < max_crf, "Invalid --min-crf & --max-crf");

    let crf_increment = crf_increment
        .unwrap_or_else(|| args.encoder.default_crf_increment())
        .max(0.001);

    let min_q = q_from_crf(*min_crf, crf_increment);
    let max_q = q_from_crf(max_crf, crf_increment);
    let mut q: u64 = (min_q + max_q) / 2;

    let mut args = sample_encode::Args {
        args: args.clone(),
        crf: 0.0,
        sample: sample.clone(),
        keep: false,
        cache: *cache,
        stdout_format: sample_encode::StdoutFormat::Json,
        vmaf: vmaf.clone(),
    };

    bar.set_length(BAR_LEN);
    let sample_bar = ProgressBar::hidden();
    let mut crf_attempts = Vec::new();

    for run in 1.. {
        // how much we're prepared to go higher than the min-vmaf
        let higher_tolerance = match thorough {
            true => 0.05,
            // increment 1.0 => +0.1, +0.2, +0.4, +0.8 ..
            // increment 0.1 => +0.1, +0.1, +0.1, +0.16 ..
            _ => (crf_increment * 2_f32.powi(run as i32 - 1) * 0.1).max(0.1),
        };
        args.crf = q.to_crf(crf_increment);
        bar.set_message(format!("sampling crf {}, ", TerseF32(args.crf)));
        let mut sample_task = tokio::task::spawn_local(sample_encode::run(
            args.clone(),
            input_probe.clone(),
            sample_bar.clone(),
        ));

        let sample_task = loop {
            match tokio::time::timeout(Duration::from_millis(100), &mut sample_task).await {
                Err(_) => {
                    let sample_progress = sample_bar.position() as f64
                        / sample_bar.length().unwrap_or(1).max(1) as f64;
                    bar.set_position(guess_progress(run, sample_progress, *thorough) as _);
                }
                Ok(o) => {
                    sample_bar.set_position(0);
                    break o;
                }
            }
        };

        let sample = Sample {
            crf_increment,
            q,
            enc: sample_task??,
        };
        let from_cache = sample.enc.from_cache;
        crf_attempts.push(sample.clone());
        let sample_small_enough = sample.enc.encode_percent <= *max_encoded_percent as _;

        if sample.enc.vmaf > *min_vmaf {
            // good
            if sample_small_enough && sample.enc.vmaf < min_vmaf + higher_tolerance {
                return Ok(sample);
            }
            let u_bound = crf_attempts
                .iter()
                .filter(|s| s.q > sample.q)
                .min_by_key(|s| s.q);

            match u_bound {
                Some(upper) if upper.q == sample.q + 1 => {
                    ensure_or_no_good_crf!(sample_small_enough, sample);
                    return Ok(sample);
                }
                Some(upper) => {
                    q = vmaf_lerp_q(*min_vmaf, upper, &sample);
                }
                None if sample.q == max_q => {
                    ensure_or_no_good_crf!(sample_small_enough, sample);
                    return Ok(sample);
                }
                None if run == 1 && sample.q + 1 < max_q => {
                    q = (sample.q + max_q) / 2;
                }
                None => q = max_q,
            };
        } else {
            // not good enough
            if !sample_small_enough || sample.q == min_q {
                sample.print_attempt(&bar, *min_vmaf, *max_encoded_percent, *quiet, from_cache);
                ensure_or_no_good_crf!(false, sample);
            }

            let l_bound = crf_attempts
                .iter()
                .filter(|s| s.q < sample.q)
                .max_by_key(|s| s.q);

            match l_bound {
                Some(lower) if lower.q + 1 == sample.q => {
                    sample.print_attempt(&bar, *min_vmaf, *max_encoded_percent, *quiet, from_cache);
                    let lower_small_enough = lower.enc.encode_percent <= *max_encoded_percent as _;
                    ensure_or_no_good_crf!(lower_small_enough, sample);
                    return Ok(lower.clone());
                }
                Some(lower) => {
                    q = vmaf_lerp_q(*min_vmaf, &sample, lower);
                }
                None if run == 1 && sample.q > min_q + 1 => {
                    q = (min_q + sample.q) / 2;
                }
                None => q = min_q,
            };
        }
        sample.print_attempt(&bar, *min_vmaf, *max_encoded_percent, *quiet, from_cache);
    }
    unreachable!();
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub enc: sample_encode::Output,
    pub crf_increment: f32,
    pub q: u64,
}

impl Sample {
    pub fn crf(&self) -> f32 {
        self.q.to_crf(self.crf_increment)
    }

    fn print_attempt(
        &self,
        bar: &ProgressBar,
        min_vmaf: f32,
        max_encoded_percent: f32,
        quiet: bool,
        from_cache: bool,
    ) {
        if quiet {
            return;
        }
        let crf_label = style("- crf").dim();
        let mut crf = style(TerseF32(self.crf()));
        let vmaf_label = style("VMAF").dim();
        let mut vmaf = style(self.enc.vmaf);
        let mut percent = style!("{:.0}%", self.enc.encode_percent);
        let open = style("(").dim();
        let close = style(")").dim();
        let cache_msg = match from_cache {
            true => style(" (cache)").dim(),
            false => style(""),
        };

        if self.enc.vmaf < min_vmaf {
            crf = crf.red().bright();
            vmaf = vmaf.red().bright();
        }
        if self.enc.encode_percent > max_encoded_percent as _ {
            crf = crf.red().bright();
            percent = percent.red().bright();
        }

        let msg =
            format!("{crf_label} {crf} {vmaf_label} {vmaf:.2} {open}{percent}{close}{cache_msg}");
        if atty::is(atty::Stream::Stderr) {
            bar.println(msg);
        } else {
            eprintln!("{msg}");
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum StdoutFormat {
    Human,
}

impl StdoutFormat {
    fn print_result(self, sample: &Sample, image: bool) {
        match self {
            Self::Human => {
                let crf = style(TerseF32(sample.crf())).bold().green();
                let enc = &sample.enc;
                let vmaf = style(enc.vmaf).bold().green();
                let size = style(HumanBytes(enc.predicted_encode_size)).bold().green();
                let percent = style!("{}%", enc.encode_percent.round()).bold().green();
                let time = style(HumanDuration(enc.predicted_encode_time)).bold();
                let enc_description = match image {
                    true => "image",
                    false => "video stream",
                };
                println!(
                    "crf {crf} VMAF {vmaf:.2} predicted {enc_description} size {size} ({percent}) taking {time}"
                );
            }
        }
    }
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
fn vmaf_lerp_q(min_vmaf: f32, worse_q: &Sample, better_q: &Sample) -> u64 {
    assert!(
        worse_q.enc.vmaf <= min_vmaf
            && worse_q.enc.vmaf < better_q.enc.vmaf
            && worse_q.q > better_q.q,
        "invalid vmaf_lerp_crf usage: ({min_vmaf}, {worse_q:?}, {better_q:?})"
    );

    let vmaf_diff = better_q.enc.vmaf - worse_q.enc.vmaf;
    let vmaf_factor = (min_vmaf - worse_q.enc.vmaf) / vmaf_diff;

    let q_diff = worse_q.q - better_q.q;
    let lerp = (worse_q.q as f32 - q_diff as f32 * vmaf_factor).round() as u64;
    lerp.clamp(better_q.q + 1, worse_q.q - 1)
}

/// sample_progress: [0, 1]
fn guess_progress(run: usize, sample_progress: f64, thorough: bool) -> f64 {
    let total_runs_guess = match () {
        // Guess 6 iterations for a "thorough" search
        _ if thorough && run < 7 => 6.0,
        // Guess 4 iterations initially
        _ if run < 5 => 4.0,
        // Otherwise guess next will work
        _ => run as f64,
    };
    ((run - 1) as f64 + sample_progress) * BAR_LEN as f64 / total_runs_guess
}

/// Calculate "q" as a quality value integer multiple of crf.
///
/// * crf=33.5, inc=0.1 -> q=335
/// * crf=27, inc=1 -> q=27
#[inline]
fn q_from_crf(crf: f32, crf_increment: f32) -> u64 {
    (f64::from(crf) / f64::from(crf_increment)).round() as _
}

trait QualityValue {
    fn to_crf(self, crf_increment: f32) -> f32;
}
impl QualityValue for u64 {
    #[inline]
    fn to_crf(self, crf_increment: f32) -> f32 {
        ((self as f64) * f64::from(crf_increment)) as _
    }
}

#[test]
fn q_crf_conversions() {
    assert_eq!(q_from_crf(33.5, 0.1), 335);
    assert_eq!(q_from_crf(27.0, 1.0), 27);
}
