use crate::{
    command::{
        crf_search,
        encode::{self, default_output_from},
        PROGRESS_CHARS,
    },
    console_ext::style,
};
use clap::Parser;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

/// Automatically determine the best crf to deliver the min-vmaf and use it to
/// encode a video.
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
    pub encode: EncodeArgs,
}

/// Encoding args that also apply to command encode.
#[derive(Parser)]
pub struct EncodeArgs {
    /// Output file, by default the same as input with `.av1` before the extension.
    ///
    /// E.g. if unspecified: -i vid.mp4 --> vid.av1.mp4
    #[clap(short, long)]
    pub output: Option<PathBuf>,

    /// Set the output ffmpeg audio codec. See https://ffmpeg.org/ffmpeg.html#Audio-Options.
    ///
    /// By default when the input & output file extension match 'copy' is used, otherwise
    /// 'libopus' is used.
    #[clap(long = "acodec")]
    pub audio_codec: Option<String>,

    /// Set the output audio quality. See https://ffmpeg.org/ffmpeg.html#Audio-Options.
    #[clap(long = "aq")]
    pub audio_quality: Option<String>,
}

pub async fn auto_encode(Args { mut search, encode }: Args) -> anyhow::Result<()> {
    search.quiet = true;
    let defaulting_output = encode.output.is_none();
    let output = encode
        .output
        .unwrap_or_else(|| default_output_from(&search.input));

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );

    bar.set_prefix("Searching");
    // bar.enable_steady_tick(100);
    if defaulting_output {
        bar.println(style!("Encoding {:?}", output).dim().to_string());
    }

    let best = crf_search::run(&search, bar.clone()).await?;

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
            input: search.input,
            crf: best.crf,
            preset: search.preset,
            encode: EncodeArgs {
                output: Some(output),
                ..encode
            },
        },
        &bar,
    )
    .await
}
