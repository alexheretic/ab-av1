//! vmaf logic
use crate::ffmpeg::FfmpegProgress;
use anyhow::{anyhow, Context};
use std::path::Path;
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Calculate the VMAF at 24fps.
pub fn run(original: &Path, distorted: &Path) -> anyhow::Result<impl Stream<Item = VmafOut>> {
    let out: ProcessChunkStream = match distorted.extension().and_then(|e| e.to_str()) {
        // `-r 24` seems to work better for .ivf samples
        Some("ivf") => Command::new("ffmpeg")
            .arg("-r")
            .arg("24")
            .arg("-i")
            .arg(distorted)
            .arg("-r")
            .arg("24")
            .arg("-i")
            .arg(original)
            .arg("-lavfi")
            .arg("libvmaf")
            .arg("-f")
            .arg("null")
            .arg("-")
            .try_into()
            .context("ffmpeg vmaf")?,
        _ => Command::new("ffmpeg")
            .arg("-i")
            .arg(distorted)
            .arg("-i")
            .arg(original)
            .arg("-lavfi")
            .arg("libvmaf")
            .arg("-f")
            .arg("null")
            .arg("-")
            .try_into()
            .context("ffmpeg vmaf")?,
    };

    Ok(out.filter_map(|item| match item {
        Item::Stderr(chunk) => VmafOut::try_from_chunk(&chunk),
        Item::Stdout(_) => None,
        Item::Done(code) => match code {
            Ok(c) if c.success() => None,
            Ok(c) => Some(VmafOut::Err(anyhow!(
                "ffmpeg vmaf exit code {:?}",
                c.code()
            ))),
            Err(err) => Some(VmafOut::Err(err.into())),
        },
    }))
}

#[derive(Debug)]
pub enum VmafOut {
    Progress(FfmpegProgress),
    Done(f32),
    Err(anyhow::Error),
}

impl VmafOut {
    fn try_from_chunk(chunk: &[u8]) -> Option<Self> {
        let out = String::from_utf8_lossy(chunk);
        if let Some(idx) = out.find("VMAF score: ") {
            return Some(Self::Done(
                out[idx + "VMAF score: ".len()..].trim().parse().ok()?,
            ));
        }
        if let Some(progress) = FfmpegProgress::try_parse(&out) {
            return Some(Self::Progress(progress));
        }
        None
    }
}
