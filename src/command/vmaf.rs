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

/// Simple full calculation of VMAF score distorted file vs original file.
#[derive(Parser)]
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
    let distorted_is_image = dprobe.is_probably_an_image();
    let duration = dprobe
        .duration
        .or_else(|_| ffprobe::probe(&reference).duration);
    if let Ok(d) = duration {
        bar.set_length(d.as_secs().max(1));
    }

    let mut vmaf = vmaf::run(
        &reference,
        reference_vfilter.as_deref(),
        &distorted,
        &vmaf.ffmpeg_lavfi(dprobe.resolution),
        match distorted_is_image {
            // 10le doesn't seem to work well for images
            true => PixelFormat::Yuv420p,
            false => PixelFormat::Yuv420p10le,
        },
    )?;
    let mut vmaf_score = -1.0;
    while let Some(vmaf) = vmaf.next().await {
        match vmaf {
            VmafOut::Done(score) => {
                vmaf_score = score;
                break;
            }
            VmafOut::Progress(FfmpegOut::Progress { time, fps, .. }) => {
                if fps > 0.0 {
                    bar.set_message(format!("vmaf {fps} fps, "));
                }
                if duration.is_ok() {
                    bar.set_position(time.as_secs());
                }
            }
            VmafOut::Progress(_) => {}
            VmafOut::Err(e) => return Err(e),
        }
    }
    bar.finish();

    println!("{vmaf_score}");
    Ok(())
}
