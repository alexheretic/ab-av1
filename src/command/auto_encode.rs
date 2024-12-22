use crate::{
    command::{
        args, crf_search,
        encode::{self, default_output_name},
        sample_encode::{self, Work},
        PROGRESS_CHARS,
    },
    console_ext::style,
    ffprobe,
    float::TerseF32,
    temporary,
};
use anyhow::Context;
use clap::Parser;
use console::style;
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::{pin::pin, sync::Arc, time::Duration};

const BAR_LEN: u64 = 1024 * 1024 * 1024;

/// Automatically determine the best crf to deliver the min-vmaf and use it to encode a video or image.
///
/// Two phases:
/// * crf-search to determine the best --crf value
/// * ffmpeg & SvtAv1EncApp to encode using the settings
///
/// Use -v to print per-crf results.
/// Use -vv to print per-sample results.
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
#[group(skip)]
pub struct Args {
    #[clap(flatten)]
    pub search: crf_search::Args,

    #[clap(flatten)]
    pub encode: args::EncodeToOutput,
}

pub async fn auto_encode(Args { mut search, encode }: Args) -> anyhow::Result<()> {
    const SPINNER_RUNNING: &str =
        "{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg}eta {eta})";
    const SPINNER_FINISHED: &str =
        "{spinner:.cyan.bold} {elapsed_precise:.bold} {prefix} {wide_bar:.cyan/blue} ({msg})";

    let defaulting_output = encode.output.is_none();
    let input_probe = Arc::new(ffprobe::probe(&search.args.input));

    let output = encode.output.unwrap_or_else(|| {
        default_output_name(
            &search.args.input,
            &search.args.encoder,
            input_probe.is_image,
        )
    });
    search.sample.set_extension_from_output(&output);

    let bar = ProgressBar::new(BAR_LEN).with_style(
        ProgressStyle::default_bar()
            .template(SPINNER_RUNNING)?
            .progress_chars(PROGRESS_CHARS),
    );
    bar.enable_steady_tick(Duration::from_millis(100));

    if defaulting_output {
        let out = shell_escape::escape(output.display().to_string().into());
        bar.println(style!("Encoding {out}").dim().to_string());
    }

    let min_score = search.min_score();
    let max_encoded_percent = search.max_encoded_percent;
    let enc_args = search.args.clone();
    let thorough = search.thorough;
    let verbose = search.verbose;

    let mut crf_search = pin!(crf_search::run(search, input_probe.clone()));
    let mut best = None;
    while let Some(update) = crf_search.next().await {
        match update {
            Err(err) => {
                if let crf_search::Error::NoGoodCrf { last } = &err {
                    // show last sample attempt in progress bar
                    bar.set_style(
                        ProgressStyle::default_bar()
                            .template(SPINNER_FINISHED)?
                            .progress_chars(PROGRESS_CHARS),
                    );
                    let mut vmaf = style(last.enc.score);
                    if last.enc.score < min_score {
                        vmaf = vmaf.red();
                    }
                    let mut percent = style!("{:.0}%", last.enc.encode_percent);
                    if last.enc.encode_percent > max_encoded_percent as _ {
                        percent = percent.red();
                    }
                    let score_kind = last.enc.score_kind;
                    bar.finish_with_message(format!("{score_kind} {vmaf:.2}, size {percent}"));
                }
                bar.finish();
                return Err(err.into());
            }
            Ok(crf_search::Update::Status {
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
            }) => {
                bar.set_position(crf_search::guess_progress(crf_run, progress, thorough) as _);
                let crf = TerseF32(crf);
                match full_pass {
                    true => bar.set_prefix(format!("crf {crf} full pass")),
                    false => bar.set_prefix(format!("crf {crf} {sample}/{samples}")),
                }
                match work {
                    Work::Encode if fps <= 0.0 => bar.set_message("encoding,  "),
                    Work::Encode => bar.set_message(format!("enc {fps} fps, ")),
                    Work::Vmaf if fps <= 0.0 => bar.set_message("vmaf,       "),
                    Work::Vmaf => bar.set_message(format!("vmaf {fps} fps, ")),
                    Work::Xpsnr if fps <= 0.0 => bar.set_message("xpsnr,      "),
                    Work::Xpsnr => bar.set_message(format!("xpsnr {fps} fps, ")),
                }
            }
            Ok(crf_search::Update::SampleResult {
                crf,
                sample,
                result,
            }) => {
                if verbose
                    .log_level()
                    .is_some_and(|lvl| lvl > log::Level::Warn)
                {
                    result.print_attempt(&bar, sample, Some(crf))
                }
            }
            Ok(crf_search::Update::RunResult(result)) => {
                if verbose
                    .log_level()
                    .is_some_and(|lvl| lvl > log::Level::Error)
                {
                    result.print_attempt(&bar, min_score, max_encoded_percent)
                }
            }
            Ok(crf_search::Update::Done(result)) => best = Some(result),
        }
    }
    let best = best.context("no crf-search best?")?;

    bar.set_style(
        ProgressStyle::default_bar()
            .template(SPINNER_FINISHED)?
            .progress_chars(PROGRESS_CHARS),
    );
    bar.finish_with_message(format!(
        "{} {:.2}, size {}",
        best.enc.score_kind,
        style(best.enc.score).green(),
        style(format!("{:.0}%", best.enc.encode_percent)).green(),
    ));
    temporary::clean_all().await;

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template(SPINNER_RUNNING)?
            .progress_chars(PROGRESS_CHARS),
    );
    bar.set_prefix("Encoding");
    bar.enable_steady_tick(Duration::from_millis(100));

    encode::run(
        encode::Args {
            args: enc_args,
            crf: best.crf(),
            encode: args::EncodeToOutput {
                output: Some(output),
                ..encode
            },
        },
        input_probe,
        &bar,
    )
    .await
}
