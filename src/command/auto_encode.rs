use crate::{
    command::{crf_search, encode, PROGRESS_CHARS},
    console_ext::style,
};
use clap::Parser;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

/// Automatically determining the best crf & use it to encode a video.
///
/// Two phases:
/// * crf-search to determine the best --crf value
/// * ffmpeg & SvtAv1EncApp to encode using the settings
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub crf_search: crf_search::Args,

    /// Output file, by default the same as input with `.av1.mp4` extension.
    #[clap(short, long)]
    pub output: Option<PathBuf>,
}

pub async fn auto_encode(mut args: Args) -> anyhow::Result<()> {
    args.crf_search.quiet = true;
    let defaulting_output = args.output.is_none();
    let output = args
        .output
        .unwrap_or_else(|| args.crf_search.input.with_extension("av1.mp4"));

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );

    bar.set_prefix("Searching");
    bar.enable_steady_tick(100);
    if defaulting_output {
        bar.println(style!("Encoding {:?}", output).dim().to_string());
    }

    let best = crf_search::run(&args.crf_search, bar.clone()).await?;

    bar.finish_with_message(format!(
        "crf {}, VMAF {:.2}, ",
        style(best.crf).green(),
        style(best.enc.vmaf).green()
    ));

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );
    bar.set_prefix("Encoding ");
    bar.enable_steady_tick(100);

    encode::run(
        encode::Args {
            input: args.crf_search.input,
            crf: best.crf,
            preset: args.crf_search.preset,
            output: Some(output),
        },
        &bar,
    )
    .await
}
