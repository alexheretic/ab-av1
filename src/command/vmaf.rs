use crate::{
    command::{
        PROGRESS_CHARS,
        args::{self, PixelFormat},
    },
    ffprobe,
    log::ProgressLogger,
    process::{CmdError, FfmpegOut},
    vmaf::{self, VmafOut},
};
use anyhow::Context;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use log::warn;
use std::{
    path::{Path, PathBuf},
    pin::pin,
    sync::atomic::Ordering,
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
    let rprobe = ffprobe::probe(&reference);
    let nframes = dprobe.nframes().or_else(|_| rprobe.nframes());
    let duration = dprobe.duration.as_ref().or(rprobe.duration.as_ref());
    if let Ok(nframes) = nframes {
        bar.set_length(nframes);
    }

    let (lavfi, v0_lavfi) = vmaf.ffmpeg_lavfi(
        dprobe.resolution,
        PixelFormat::opt_max(dprobe.pixel_format(), rprobe.pixel_format()),
        score.reference_vfilter.as_deref(),
    );
    let vmaf_score =
        match do_vmaf_progress(&reference, &distorted, &bar, &nframes, duration, &lavfi).await {
            Ok(v) => v,
            Err(e)
                if let Some(v0_lavfi) = &v0_lavfi
                    && let Some(e) = e.downcast_ref::<CmdError>()
                    && e.could_be_caused_by_vmaf_model()
                    && {
                        warn!("v1 vmaf model not supported, falling back to v0");
                        args::DEFAULT_V0_MODELS.store(true, Ordering::Relaxed);
                        true
                    }
                    && let Ok(Some(score)) = do_vmaf_progress(
                        &reference, &distorted, &bar, &nframes, duration, v0_lavfi,
                    )
                    .await =>
            {
                Some(score)
            }
            Err(e) => return Err(e),
        };
    bar.finish();

    println!("{}", vmaf_score.context("no vmaf score")?);
    Ok(())
}

async fn do_vmaf_progress(
    reference: &Path,
    distorted: &Path,
    bar: &ProgressBar,
    nframes: &Result<u64, ffprobe::ProbeError>,
    duration: Result<&Duration, &ffprobe::ProbeError>,
    lavfi: &args::VmafFfmpeg,
) -> anyhow::Result<Option<f32>> {
    let mut vmaf_score = None;
    let mut vmaf = pin!(vmaf::run(
        reference,
        distorted,
        &lavfi.filter_complex,
        lavfi.fps,
    )?);
    let mut logger = ProgressLogger::new(module_path!(), Instant::now());
    while let Some(vmaf) = vmaf.next().await {
        match vmaf {
            VmafOut::Done(score) => {
                vmaf_score = Some(score);
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
            VmafOut::Err(e) => return Err(e.into()),
        }
    }
    Ok(vmaf_score)
}
