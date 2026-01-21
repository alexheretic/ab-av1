use crate::{
    command::{
        PROGRESS_CHARS,
        args::{self, PixelFormat},
    },
    ffprobe,
    log::ProgressLogger,
    process::FfmpegOut,
    xpsnr::{self, XpsnrOut},
};
use anyhow::Context;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    fmt::Write,
    path::PathBuf,
    pin::pin,
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
    let rprobe = ffprobe::probe(&reference);
    let nframes = dprobe.nframes().or_else(|_| rprobe.nframes());
    let duration = dprobe.duration.as_ref().or(rprobe.duration.as_ref());
    if let Ok(nframes) = nframes {
        bar.set_length(nframes);
    }

    let mut xpsnr_out = pin!(xpsnr::run(
        &reference,
        &distorted,
        &lavfi(
            score.reference_vfilter.as_deref(),
            PixelFormat::opt_max(dprobe.pixel_format(), rprobe.pixel_format()),
        ),
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

pub fn lavfi(ref_vfilter: Option<&str>, pix_fmt: Option<PixelFormat>) -> String {
    let mut lavfi = String::from("[0:v]");
    if let Some(pix_fmt) = pix_fmt {
        _ = write!(&mut lavfi, "format={pix_fmt}");
    }
    if let Some(vf) = ref_vfilter {
        if pix_fmt.is_some() {
            lavfi.push(',');
        }
        lavfi.push_str(vf);
    }
    lavfi.push_str("[ref];[1:v]");
    if let Some(pix_fmt) = pix_fmt {
        _ = write!(&mut lavfi, "format={pix_fmt}");
    }
    lavfi.push_str("[dis];[ref][dis]xpsnr=stats_file=-");
    lavfi
}

#[test]
fn test_lavfi_default() {
    assert_eq!(
        lavfi(None, None),
        "[0:v][ref];[1:v][dis];[ref][dis]xpsnr=stats_file=-"
    );
}

#[test]
fn test_lavfi_ref_vfilter() {
    assert_eq!(
        lavfi(Some("scale=1280:-1"), None),
        "[0:v]scale=1280:-1[ref];\
         [1:v][dis];\
         [ref][dis]xpsnr=stats_file=-"
    );
}

#[test]
fn test_lavfi_pixel_format() {
    assert_eq!(
        lavfi(None, Some(PixelFormat::Yuv420p10le)),
        "[0:v]format=yuv420p10le[ref];\
         [1:v]format=yuv420p10le[dis];\
         [ref][dis]xpsnr=stats_file=-"
    );
}

#[test]
fn test_lavfi_all() {
    assert_eq!(
        lavfi(Some("scale=640:-1"), Some(PixelFormat::Yuv420p10le)),
        "[0:v]format=yuv420p10le,scale=640:-1[ref];\
         [1:v]format=yuv420p10le[dis];\
         [ref][dis]xpsnr=stats_file=-"
    );
}
