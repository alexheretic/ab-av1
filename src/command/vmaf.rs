use crate::{command::PROGRESS_CHARS, ffprobe, sample::FfmpegProgress, vmaf, vmaf::VmafOut};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio_stream::StreamExt;

/// Simple full calculation of VMAF score distorted file vs original file.
#[derive(Parser)]
pub struct Args {
    /// Original video file.
    #[clap(long)]
    original: PathBuf,
    /// Re-encoded/distorted video file.
    #[clap(long)]
    distorted: PathBuf,
}

pub async fn vmaf(
    Args {
        original,
        distorted,
    }: Args,
) -> anyhow::Result<()> {
    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(100);
    bar.set_message("vmaf running, ");

    let duration = ffprobe::probe(&original).map(|p| p.duration);
    if let Ok(d) = duration {
        bar.set_length(d.as_secs());
    }

    let mut vmaf = vmaf::run(&original, &distorted)?;
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
