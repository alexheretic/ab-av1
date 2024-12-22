use crate::{
    command::PROGRESS_CHARS,
    ffprobe,
    log::ProgressLogger,
    process::FfmpegOut,
    xpsnr::{self, XpsnrOut},
};
use anyhow::Context;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    path::PathBuf,
    pin::pin,
    sync::LazyLock,
    time::{Duration, Instant},
};
use tokio_stream::StreamExt;

/// Full XPSNR score calculation, distorted file vs reference file.
/// Works with videos and images.
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
#[group(skip)]
pub struct Args {
    /// Reference video file.
    #[arg(long)]
    pub reference: PathBuf,

    /// Re-encoded/distorted video file.
    #[arg(long)]
    pub distorted: PathBuf,
}

pub async fn xpsnr(
    Args {
        reference,
        distorted,
    }: Args,
) -> anyhow::Result<()> {
    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(Duration::from_millis(100));
    bar.set_message("xpsnr running, ");

    let dprobe = ffprobe::probe(&distorted);
    let rprobe = LazyLock::new(|| ffprobe::probe(&reference));
    let nframes = dprobe.nframes().or_else(|_| rprobe.nframes());
    let duration = dprobe
        .duration
        .as_ref()
        .or_else(|_| rprobe.duration.as_ref());
    if let Ok(nframes) = nframes {
        bar.set_length(nframes);
    }

    let mut xpsnr_out = pin!(xpsnr::run(&reference, &distorted)?);
    let mut logger = ProgressLogger::new(module_path!(), Instant::now());
    let mut score = None;
    while let Some(next) = xpsnr_out.next().await {
        match next {
            XpsnrOut::Done(s) => {
                score = Some(s);
                break;
            }
            XpsnrOut::Progress(FfmpegOut::Progress {
                frame, fps, time, ..
            }) => {
                if fps > 0.0 {
                    bar.set_message(format!("xpsnr {fps} fps, "));
                }
                if nframes.is_ok() {
                    bar.set_position(frame);
                }
                if let Ok(total) = duration {
                    logger.update(*total, time, fps);
                }
            }
            XpsnrOut::Progress(FfmpegOut::StreamSizes { .. }) => {}
            XpsnrOut::Err(e) => return Err(e),
        }
    }
    bar.finish();

    println!("{}", score.context("no xpsnr score")?);
    Ok(())
}
