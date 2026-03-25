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
            xpsnr.xpsnr_pix_format.or(PixelFormat::opt_max(
                dprobe.pixel_format(),
                rprobe.pixel_format()
            )),
        ),
        xpsnr.fps(),
    )?);
    let mut logger = ProgressLogger::new(module_path!(), Instant::now());
    let mut score = None;
    while let Some(next) = xpsnr_out.next().await {
        match next {
            XpsnrOut::Done(s) => {
                score = Some(s);
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
    /// Add filter to `lavfi`, if necessary. If no filter added return `old_name`.
    /// Otherwise return `new_name`.
    fn add_filter(
        lavfi: &mut String,
        old_name: &'static str,
        new_name: &'static str,
        vfilter: Option<&str>,
        pix_fmt: Option<PixelFormat>,
    ) -> &'static str {
        if vfilter.is_none() && pix_fmt.is_none() {
            return old_name;
        }

        lavfi.push_str(old_name);
        if let Some(pix_fmt) = pix_fmt {
            _ = write!(lavfi, "format={pix_fmt}");
        }
        if let Some(vf) = vfilter {
            if pix_fmt.is_some() {
                lavfi.push(',');
            }
            lavfi.push_str(vf);
        }
        lavfi.push_str(new_name);
        lavfi.push(';');
        new_name
    }

    let mut lavfi = String::new();

    let ref_stream = add_filter(&mut lavfi, "[0:v]", "[ref]", ref_vfilter, pix_fmt);
    let dis_stream = add_filter(&mut lavfi, "[1:v]", "[dis]", None, pix_fmt);
    lavfi.push_str(ref_stream);
    lavfi.push_str(dis_stream);
    lavfi.push_str("xpsnr");
    lavfi
}

#[test]
fn test_lavfi_default() {
    assert_eq!(lavfi(None, None), "[0:v][1:v]xpsnr");
}

#[test]
fn test_lavfi_ref_vfilter() {
    assert_eq!(
        lavfi(Some("scale=1280:-1"), None),
        "[0:v]scale=1280:-1[ref];\
         [ref][1:v]xpsnr"
    );
}

#[test]
fn test_lavfi_pixel_format() {
    assert_eq!(
        lavfi(None, Some(PixelFormat::Yuv420p10le)),
        "[0:v]format=yuv420p10le[ref];\
         [1:v]format=yuv420p10le[dis];\
         [ref][dis]xpsnr"
    );
}

#[test]
fn test_lavfi_all() {
    assert_eq!(
        lavfi(Some("scale=640:-1"), Some(PixelFormat::Yuv420p10le)),
        "[0:v]format=yuv420p10le,scale=640:-1[ref];\
         [1:v]format=yuv420p10le[dis];\
         [ref][dis]xpsnr"
    );
}
