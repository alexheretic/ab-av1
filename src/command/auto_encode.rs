use crate::{
    command::{
        args, crf_search,
        encode::{self, default_output_from},
        PROGRESS_CHARS,
    },
    console_ext::style,
    temporary,
};
use clap::Parser;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

/// Automatically determine the best crf to deliver the min-vmaf and use it to encode a video.
///
/// Two phases:
/// * crf-search to determine the best --crf value
/// * ffmpeg & SvtAv1EncApp to encode using the settings
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub search: crf_search::Args,

    #[clap(flatten)]
    pub encode: args::EncodeToOutput,
}

pub async fn auto_encode(Args { mut search, encode }: Args) -> anyhow::Result<()> {
    search.quiet = true;
    let defaulting_output = encode.output.is_none();
    let output = encode
        .output
        .unwrap_or_else(|| default_output_from(&search.svt.input));

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );

    bar.set_prefix("Searching");
    if defaulting_output {
        bar.println(style!("Encoding {:?}", output).dim().to_string());
    }

    let best = match crf_search::run(&search, bar.clone()).await {
        Ok(best) => best,
        Err(err) => {
            bar.finish();
            return match err {
                crf_search::Error::NoGoodCrf { last } => {
                    let attempt = last.attempt_string(search.min_vmaf, search.max_encoded_percent);
                    Err(anyhow::anyhow!(
                        "Failed to find a suitable crf, last attempt {attempt}"
                    ))
                }
                crf_search::Error::Other(err) => Err(err),
            };
        }
    };
    bar.set_style(ProgressStyle::default_bar()
        .template("{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg})")
        .progress_chars(PROGRESS_CHARS));
    bar.finish_with_message(format!(
        "crf {}, VMAF {:.2}, size {}",
        style(best.crf).green(),
        style(best.enc.vmaf).green(),
        style(format!("{:.0}%", best.enc.predicted_encode_percent)).green(),
    ));
    temporary::clean_all().await;

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );
    bar.set_prefix("Encoding ");
    bar.enable_steady_tick(100);

    encode::run(
        encode::Args {
            svt: search.svt,
            crf: best.crf,
            encode: args::EncodeToOutput {
                output: Some(output),
                ..encode
            },
        },
        &bar,
    )
    .await
}
