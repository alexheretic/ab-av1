use crate::{
    command::{
        args::{self, PixelFormat},
        PROGRESS_CHARS,
    },
    ffprobe,
    process::FfmpegProgress,
    vmaf,
    vmaf::VmafOut,
};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio_stream::StreamExt;

/// Simple full calculation of VMAF score distorted file vs original file.
#[derive(Parser)]
pub struct Args {
    /// Reference video file.
    #[clap(long)]
    pub reference: PathBuf,

    /// Re-encoded/distorted video file.
    #[clap(long)]
    pub distorted: PathBuf,

    #[clap(flatten)]
    pub vmaf: args::Vmaf,
}

pub async fn vmaf(
    Args {
        reference: original,
        distorted,
        vmaf,
    }: Args,
) -> anyhow::Result<()> {
    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(100);
    bar.set_message("vmaf running, ");

    let duration = ffprobe::probe(&original).duration;
    if let Ok(d) = duration {
        bar.set_length(d.as_secs());
    }

    let mut vmaf = vmaf::run(
        &original,
        &distorted,
        &vmaf.ffmpeg_lavfi(),
        PixelFormat::Yuv420p10le,
    )?;
    let mut vmaf_score = -1.0;
    while let Some(vmaf) = vmaf.next().await {
        match vmaf {
            VmafOut::Done(score) => {
                vmaf_score = score;
                break;
            }
            VmafOut::Progress(FfmpegProgress { time, fps, .. }) => {
                if fps > 0.0 {
                    bar.set_message(format!("vmaf {fps} fps, "));
                }
                if duration.is_ok() {
                    bar.set_position(time.as_secs());
                }
            }
            VmafOut::Err(e) => return Err(e),
        }
    }
    bar.finish();

    println!("{vmaf_score}");
    Ok(())
}
