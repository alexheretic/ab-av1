use crate::{
    command::{PROGRESS_CHARS, args},
    ffprobe,
    log::ProgressLogger,
    process::FfmpegOut,
    xpsnr::{self, XpsnrOut},
};
use anyhow::Context;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    borrow::Cow,
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

    #[clap(flatten)]
    pub score: args::ScoreArgs,

    #[clap(flatten)]
    pub xpsnr: args::Xpsnr,
}

pub async fn xpsnr(
    Args {
        reference,
        distorted,
        score,
        xpsnr,
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

    let mut xpsnr_out = pin!(xpsnr::run(
        &reference,
        &distorted,
        &lavfi(score.reference_vfilter.as_deref()),
        xpsnr.fps(),
    )?);
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

pub fn lavfi(ref_vfilter: Option<&str>) -> Cow<'static, str> {
    match ref_vfilter {
        None => "xpsnr=stats_file=-".into(),
        Some(vf) => format!("[0:v]{vf}[ref];[ref][1:v]xpsnr=stats_file=-").into(),
    }
}

#[test]
fn test_lavfi_default() {
    assert_eq!(lavfi(None), "xpsnr=stats_file=-");
}

#[test]
fn test_lavfi_ref_vfilter() {
    assert_eq!(
        lavfi(Some("scale=1280:-1")),
        "[0:v]scale=1280:-1[ref];\
         [ref][1:v]xpsnr=stats_file=-"
    );
}
