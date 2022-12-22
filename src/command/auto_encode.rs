use crate::{
    command::{
        args, crf_search,
        encode::{self, default_output_name},
        PROGRESS_CHARS,
    },
    console_ext::style,
    ffprobe, temporary,
};
use clap::Parser;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::{fs, sync::Arc, time::Duration};

/// Automatically determine the best crf to deliver the min-vmaf and use it to encode a video or image.
///
/// Two phases:
/// * crf-search to determine the best --crf value
/// * ffmpeg & SvtAv1EncApp to encode using the settings
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
        "{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})";
    const SPINNER_FINISHED: &str =
        "{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg})";

    search.quiet = true;
    let defaulting_output = encode.output.is_none();
    let input_probe = Arc::new(ffprobe::probe(&search.args.input));

    // Keep image samples so we can use the best sample as the final image
    search.keep = input_probe.is_probably_an_image();

    let output = encode.output.unwrap_or_else(|| {
        default_output_name(
            &search.args.input,
            &search.args.encoder,
            input_probe.is_probably_an_image(),
        )
    });
    search.sample.set_extension_from_output(&output);

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template(SPINNER_RUNNING)?
            .progress_chars(PROGRESS_CHARS),
    );

    bar.set_prefix("Searching");
    if defaulting_output {
        let out = shell_escape::escape(output.display().to_string().into());
        bar.println(style!("Encoding {out}").dim().to_string());
    }

    let best = match crf_search::run(&search, input_probe.clone(), bar.clone()).await {
        Ok(best) => best,
        Err(err) => {
            if let crf_search::Error::NoGoodCrf { last } = &err {
                // show last sample attempt in progress bar
                bar.set_style(
                    ProgressStyle::default_bar()
                        .template(SPINNER_FINISHED)?
                        .progress_chars(PROGRESS_CHARS),
                );
                let mut vmaf = style(last.enc.vmaf);
                if last.enc.vmaf < search.min_vmaf {
                    vmaf = vmaf.red();
                }
                let mut percent = style!("{:.0}%", last.enc.encode_percent);
                if last.enc.encode_percent > search.max_encoded_percent as _ {
                    percent = percent.red();
                }
                bar.finish_with_message(format!(
                    "crf {}, VMAF {vmaf:.2}, size {percent}",
                    style(last.crf()).red(),
                ));
            }
            bar.finish();
            return Err(err.into());
        }
    };
    bar.set_style(
        ProgressStyle::default_bar()
            .template(SPINNER_FINISHED)?
            .progress_chars(PROGRESS_CHARS),
    );
    bar.finish_with_message(format!(
        "crf {}, VMAF {:.2}, size {}",
        style(best.crf()).green(),
        style(best.enc.vmaf).green(),
        style(format!("{:.0}%", best.enc.encode_percent)).green(),
    ));

    // We don't need to do a final encode for images since we already did it when searching for the best CRF
    if input_probe.is_probably_an_image() {
        let output_path_with_best_crf = &output.with_extension(format!("crf{}.avif", best.crf()));

        // Check if we actually have a real sample or if our best CRF was just read from the cache
        if output_path_with_best_crf.exists() {
            fs::rename(output_path_with_best_crf, output)?;
            temporary::clean_all().await;
            return Ok(());
        }

        // If the best CRF was read from the cache then we don't have an image to re-use, so we just continue as normal
    }

    temporary::clean_all().await;

    let bar = ProgressBar::new(12).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {prefix} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.set_prefix("Encoding ");
    bar.enable_steady_tick(Duration::from_millis(100));

    encode::run(
        encode::Args {
            args: search.args,
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
