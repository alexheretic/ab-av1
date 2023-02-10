use crate::{
    command::{
        args::{self, PixelFormat},
        PROGRESS_CHARS,
    },
    ffprobe,
    process::FfmpegOut,
    vmaf,
    vmaf::VmafOut,
};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::{path::PathBuf, time::Duration};
use tokio_stream::StreamExt;

/// Full VMAF score calculation, distorted file vs reference file.
/// Works with videos and images.
///
/// * Auto sets model version (4k or 1k) according to resolution.
/// * Auto sets `n_threads` to system threads.
/// * Auto upscales lower resolution videos to the model.
/// * Converts distorted & reference to appropriate format yuv streams before passing to vmaf.
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
#[group(skip)]
pub struct Args {
    /// Reference video file.
    #[arg(long)]
    pub reference: PathBuf,

    /// Ffmpeg video filter applied to the reference before analysis.
    /// E.g. --vfilter "scale=1280:-1,fps=24".
    #[arg(long)]
    pub reference_vfilter: Option<String>,

    /// Re-encoded/distorted video file.
    #[arg(long)]
    pub distorted: PathBuf,

    #[clap(flatten)]
    pub vmaf: args::Vmaf,
}

pub async fn vmaf(
    Args {
        reference,
        reference_vfilter,
        distorted,
        vmaf,
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
    if let Ok(nframes) = nframes {
        bar.set_length(nframes);
    }

    let mut vmaf = vmaf::run(
        &reference,
        reference_vfilter.as_deref(),
        &distorted,
        &vmaf.ffmpeg_lavfi(dprobe.resolution),
        dpix_fmt.max(rpix_fmt),
    )?;
    let mut vmaf_score = -1.0;
    while let Some(vmaf) = vmaf.next().await {
        match vmaf {
            VmafOut::Done(score) => {
                vmaf_score = score;
                break;
            }
            VmafOut::Progress(FfmpegOut::Progress { frame, fps, .. }) => {
                if fps > 0.0 {
                    bar.set_message(format!("vmaf {fps} fps, "));
                }
                if nframes.is_ok() {
                    bar.set_position(frame);
                }
            }
            VmafOut::Progress(FfmpegOut::StreamSizes { .. }) => {}
            VmafOut::Err(e) => return Err(e),
        }
    }
    bar.finish();

    println!("{vmaf_score}");
    Ok(())
}
