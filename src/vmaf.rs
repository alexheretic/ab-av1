//! vmaf logic
use crate::{
    process::{exit_ok, FfmpegProgress},
    yuv,
};
use anyhow::Context;
use std::path::Path;
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Calculate VMAF score by converting the original first to yuv.
/// This can produce more accurate results than testing directly from original source.
pub fn run(original: &Path, distorted: &Path) -> anyhow::Result<impl Stream<Item = VmafOut>> {
    let (yuv_out, yuv_pipe) = yuv::yuv4mpegpipe(original)?;
    let yuv_pipe = yuv_pipe.filter_map(VmafOut::ignore_ok);

    let vmaf: ProcessChunkStream = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg("-i")
        .arg(distorted)
        .arg("-i")
        .arg("-")
        .arg("-lavfi")
        .arg("libvmaf")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(yuv_out)
        .try_into()
        .context("ffmpeg vmaf")?;
    let vmaf = vmaf.filter_map(|item| match item {
        Item::Stderr(chunk) => VmafOut::try_from_chunk(&chunk),
        Item::Stdout(_) => None,
        Item::Done(code) => VmafOut::ignore_ok(exit_ok("ffmpeg vmaf", code)),
    });

    Ok(yuv_pipe.merge(vmaf))
}

#[derive(Debug)]
pub enum VmafOut {
    Progress(FfmpegProgress),
    Done(f32),
    Err(anyhow::Error),
}

impl VmafOut {
    fn ignore_ok<T>(result: anyhow::Result<T>) -> Option<Self> {
        match result {
            Ok(_) => None,
            Err(err) => Some(Self::Err(err)),
        }
    }

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
