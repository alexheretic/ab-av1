use crate::{
    command::{
        args::{self, PixelFormat},
        PROGRESS_CHARS,
    },
    ffprobe,
    log::ProgressLogger,
    process::FfmpegOut,
    vmaf::{self, VmafOut},
};
use anyhow::Context;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    path::PathBuf,
    pin::pin,
    time::{Duration, Instant},
};
use tokio_stream::StreamExt;

/// Full VMAF score calculation, distorted file vs reference file.
/// Works with videos and images.
///
/// * Auto sets model version (4k or 1k) according to resolution.
/// * Auto sets `n_threads` to system threads.
/// * Auto upscales lower resolution videos to the model.
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
    pub vmaf: args::Vmaf,

    #[clap(flatten)]
    pub score: args::ScoreArgs,
}

pub async fn vmaf(
    Args {
        reference,
        distorted,
        vmaf,
        score,
    }: Args,
) -> anyhow::Result<()> {
    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(Duration::from_millis(100));
    bar.set_message("vmaf running, ");

    let dprobe = ffprobe::probe(&distorted);
    let dpix_fmt = dprobe.pixel_format().unwrap_or(PixelFormat::Yuv444p10le);
    let rprobe = ffprobe::probe(&reference);
    let rpix_fmt = rprobe.pixel_format().unwrap_or(PixelFormat::Yuv444p10le);
    let nframes = dprobe.nframes().or_else(|_| rprobe.nframes());
    let duration = dprobe.duration.as_ref().or(rprobe.duration.as_ref());
    if let Ok(nframes) = nframes {
        bar.set_length(nframes);
    }

    let mut vmaf = pin!(vmaf::run(
        &reference,
        &distorted,
        &vmaf.ffmpeg_lavfi(
            dprobe.resolution,
            dpix_fmt.max(rpix_fmt),
            score.reference_vfilter.as_deref(),
        ),
        vmaf.vmaf_fps,
    )?);
    let mut logger = ProgressLogger::new(module_path!(), Instant::now());
    let mut vmaf_score = None;
    while let Some(vmaf) = vmaf.next().await {
        match vmaf {
            VmafOut::Done(score) => {
                vmaf_score = Some(score);
                break;
            }
            VmafOut::Progress(FfmpegOut::Progress {
                frame, fps, time, ..
            }) => {
                if fps > 0.0 {
                    bar.set_message(format!("vmaf {fps} fps, "));
                }
                if nframes.is_ok() {
                    bar.set_position(frame);
                }
                if let Ok(total) = duration {
                    logger.update(*total, time, fps);
                }
            }
            VmafOut::Progress(FfmpegOut::StreamSizes { .. }) => {}
            VmafOut::Err(e) => return Err(e),
        }
    }
    bar.finish();

    println!("{}", vmaf_score.context("no vmaf score")?);
    Ok(())
}
