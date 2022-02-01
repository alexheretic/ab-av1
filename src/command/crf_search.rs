use crate::{
    command::{sample_encode, PROGRESS_CHARS},
    console_ext::style,
};
use anyhow::{bail, ensure};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use std::{path::PathBuf, time::Duration};

const BAR_LEN: u64 = 1000;

/// Interpolated binary search using sample-encode to find the best crf value
/// delivering min-vmaf & max-encoded-percent.
///
/// Outputs:
/// * Best crf value
/// * Mean sample VMAF score
/// * Predicted full encode size
/// * Predicted full encode time
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    /// Input video file.
    #[clap(short, long)]
    pub input: PathBuf,

    /// Encoder preset. Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long)]
    pub preset: u8,

    /// Desired VMAF score for the
    #[clap(long, default_value_t = 95.0)]
    pub min_vmaf: f32,

    /// Maximum desired encoded size percentage of the input size.
    #[clap(long, default_value_t = 80.0)]
    pub max_encoded_percent: f32,

    /// Minimum (highest quality) crf value to try.
    #[clap(long, default_value_t = 10)]
    pub min_crf: u8,

    /// Maximum (lowest quality) crf value to try.
    #[clap(long, default_value_t = 55)]
    pub max_crf: u8,

    /// Number of 20s samples to use across the input video.
    /// More samples take longer but may provide a more accurate result.
    #[clap(long, default_value_t = 3)]
    pub samples: u64,

    #[clap(skip)]
    pub quiet: bool,
}

pub async fn crf_search(args: Args) -> anyhow::Result<()> {
    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );
    // bar.enable_steady_tick(100);

    let best = run(&args, bar.clone()).await?;

    bar.finish();

    // encode how-to hint + predictions
    eprintln!(
        "\n{} {}\n",
        style("Encode with:").dim(),
        style!(
            "ab-av1 encode -i {:?} --crf {} --preset {}",
            args.input,
            best.crf,
            args.preset,
        )
        .dim()
        .italic()
    );

    StdoutFormat::Human.print_result(&best);

    Ok(())
}

pub async fn run(
    Args {
        input,
        preset,
        min_vmaf,
        max_encoded_percent,
        min_crf,
        max_crf,
        samples,
        quiet,
    }: &Args,
    bar: ProgressBar,
) -> anyhow::Result<Sample> {
    ensure!(min_crf <= max_crf, "Invalid --min-crf & --max-crf");

    let mut args = sample_encode::Args {
        input: input.clone(),
        crf: (min_crf + max_crf) / 2,
        preset: *preset,
        samples: *samples,
        keep: false,
        stdout_format: sample_encode::StdoutFormat::Json,
    };

    bar.set_length(BAR_LEN);
    let sample_bar = ProgressBar::hidden();
    let mut crf_attempts = Vec::new();

    for run in 1.. {
        // how much we're prepared to go higher than the min-vmaf: +0.2, +0.4, +0.8, +1.6 ...
        let higher_tolerance = 2_f32.powi(run as _) * 0.1;
        bar.set_message(format!("sampling crf {}, ", args.crf));
        let mut sample_task =
            tokio::task::spawn_local(sample_encode::run(args.clone(), sample_bar.clone()));

        let sample_task = loop {
            match tokio::time::timeout(Duration::from_millis(100), &mut sample_task).await {
                Err(_) => {
                    let sample_progress =
                        sample_bar.position() as f64 / sample_bar.length().max(1) as f64;
                    bar.set_position(guess_progress(run, sample_progress) as _);
                }
                Ok(o) => {
                    sample_bar.set_position(0);
                    break o;
                }
            }
        };

        let sample = Sample {
            crf: args.crf,
            enc: sample_task??,
        };
        crf_attempts.push(sample.clone());

        if sample.enc.vmaf > *min_vmaf {
            // good
            if sample.enc.predicted_encode_percent < *max_encoded_percent as _
                && sample.enc.vmaf < min_vmaf + higher_tolerance
            {
                return Ok(sample);
            }
            let u_bound = crf_attempts
                .iter()
                .filter(|s| s.crf > sample.crf)
                .min_by_key(|s| s.crf);

            match u_bound {
                Some(upper) if upper.crf == sample.crf + 1 => {
                    return Ok(sample);
                }
                Some(upper) => {
                    args.crf = vmaf_lerp_crf(*min_vmaf, upper, &sample);
                }
                None if sample.crf == *max_crf => {
                    return Ok(sample);
                }
                None if run == 1 && sample.crf + 1 < *max_crf => {
                    args.crf = (sample.crf + max_crf) / 2;
                }
                None => args.crf = *max_crf,
            };
        } else {
            // not good enough
            if sample.enc.predicted_encode_percent > *max_encoded_percent as _
                || sample.crf == *min_crf
            {
                sample.print_attempt(&bar, *min_vmaf, *max_encoded_percent, *quiet);
                bail!("Failed to find a suitable crf");
            }

            let l_bound = crf_attempts
                .iter()
                .filter(|s| s.crf < sample.crf)
                .max_by_key(|s| s.crf);

            match l_bound {
                Some(lower) if lower.crf + 1 == sample.crf => {
                    sample.print_attempt(&bar, *min_vmaf, *max_encoded_percent, *quiet);
                    return Ok(lower.clone());
                }
                Some(lower) => {
                    args.crf = vmaf_lerp_crf(*min_vmaf, &sample, lower);
                }
                None if run == 1 && sample.crf > min_crf + 1 => {
                    args.crf = (min_crf + sample.crf) / 2;
                }
                None => args.crf = *min_crf,
            };
        }
        sample.print_attempt(&bar, *min_vmaf, *max_encoded_percent, *quiet);
    }
    unreachable!();
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub enc: sample_encode::Output,
    pub crf: u8,
}

impl Sample {
    fn print_attempt(
        &self,
        bar: &ProgressBar,
        min_vmaf: f32,
        max_encoded_percent: f32,
        quiet: bool,
    ) {
        if quiet {
            return;
        }

        let crf_label = style("- crf").dim();
        let mut crf = style(self.crf);
        let vmaf_label = style("VMAF").dim();
        let mut vmaf = style(self.enc.vmaf);
        let mut percent = style!("{:.0}%", self.enc.predicted_encode_percent);
        let open = style("(").dim();
        let close = style(")").dim();

        if self.enc.vmaf < min_vmaf {
            crf = crf.red().bright();
            vmaf = vmaf.red().bright();
        }
        if self.enc.predicted_encode_percent > max_encoded_percent as _ {
            crf = crf.red().bright();
            percent = percent.red().bright();
        }

        bar.println(format!(
            "{crf_label} {crf} {vmaf_label} {vmaf:.2} {open}{percent}{close}"
        ));
    }
}

#[derive(Debug, Clone, Copy, clap::ArgEnum)]
pub enum StdoutFormat {
    Human,
}

impl StdoutFormat {
    fn print_result(self, Sample { crf, enc, .. }: &Sample) {
        match self {
            Self::Human => {
                let crf = style(crf).bold().green();
                let vmaf = style(enc.vmaf).bold().green();
                let size = style(HumanBytes(enc.predicted_encode_size)).bold().green();
                let percent = style!("{}%", enc.predicted_encode_percent.round())
                    .bold()
                    .green();
                let time = style(HumanDuration(enc.predicted_encode_time)).bold();
                println!(
                    "crf {crf} VMAF {vmaf:.2} predicted full encode size {size} ({percent}) taking {time}"
                );
            }
        }
    }
}

/// Produce a crf value between given samples using vmaf score linear interpolation.
fn vmaf_lerp_crf(min_vmaf: f32, worse_q: &Sample, better_q: &Sample) -> u8 {
    assert!(
        worse_q.enc.vmaf <= min_vmaf
            && worse_q.enc.vmaf < better_q.enc.vmaf
            && better_q.crf < worse_q.crf + 1,
        "invalid vmaf_lerp_crf usage: {:?}, {:?}",
        worse_q,
        better_q
    );

    let vmaf_diff = better_q.enc.vmaf - worse_q.enc.vmaf;
    let vmaf_factor = (min_vmaf - worse_q.enc.vmaf) / vmaf_diff;

    let crf_diff = worse_q.crf - better_q.crf;
    let lerp = (worse_q.crf as f32 - crf_diff as f32 * vmaf_factor).round() as u8;
    lerp.max(better_q.crf + 1).min(worse_q.crf - 1)
}

/// sample_progress: [0, 1]
fn guess_progress(run: usize, sample_progress: f64) -> f64 {
    let total_runs_guess = match () {
        // Guess 4 iterations initially
        _ if run < 5 => 4.0,
        // Otherwise guess next will work
        _ => run as f64,
    };
    ((run - 1) as f64 + sample_progress) * BAR_LEN as f64 / total_runs_guess
}
