//! vmaf logic
use crate::{
    command::args::PixelFormat,
    process::{exit_ok, Chunks, CommandExt, FfmpegOut},
    yuv,
};
use anyhow::Context;
use std::{path::Path, sync::Mutex};
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Calculate VMAF score by converting the original first to yuv.
/// This can produce more accurate results than testing directly from original source.
pub fn run(
    reference: &Path,
    reference_vfilter: Option<&str>,
    distorted: &Path,
    filter_complex: &str,
    pix_fmt: PixelFormat,
) -> anyhow::Result<impl Stream<Item = VmafOut>> {
    let (yuv_out, yuv_pipe) = yuv::pipe(reference, pix_fmt, reference_vfilter)?;
    let yuv_pipe = yuv_pipe.filter_map(VmafOut::ignore_ok);

    // If possible convert distorted to yuv, in some cases this fixes inaccuracy
    #[cfg(unix)]
    let (distorted_fifo, distorted_yuv_pipe) = yuv::unix::pipe_to_fifo(distorted, pix_fmt)?;
    #[cfg(unix)]
    let (distorted, yuv_pipe) = (
        &distorted_fifo,
        yuv_pipe.merge(distorted_yuv_pipe.filter_map(VmafOut::ignore_ok)),
    );

    let vmaf: ProcessChunkStream = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg2("-i", distorted)
        .arg2("-i", "-")
        .arg2("-filter_complex", filter_complex)
        .arg2("-f", "null")
        .arg("-")
        .stdin(yuv_out)
        .try_into()
        .context("ffmpeg vmaf")?;

    let chunks: Mutex<Chunks> = <_>::default();
    let vmaf = vmaf.filter_map(move |item| match item {
        Item::Stderr(chunk) => VmafOut::try_from_chunk(&chunk, &chunks),
        Item::Stdout(_) => None,
        Item::Done(code) => VmafOut::ignore_ok(exit_ok("ffmpeg vmaf", code)),
    });

    Ok(yuv_pipe.merge(vmaf))
}

#[derive(Debug)]
pub enum VmafOut {
    Progress(FfmpegOut),
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

    fn try_from_chunk(chunk: &[u8], chunks: &Mutex<Chunks>) -> Option<Self> {
        let mut chunks = chunks.lock().unwrap();
        chunks.push(chunk);
        let line = chunks.last_line();

        if let Some(idx) = line.find("VMAF score: ") {
            return Some(Self::Done(
                line[idx + "VMAF score: ".len()..].trim().parse().ok()?,
            ));
        }
        if let Some(progress) = FfmpegOut::try_parse(line) {
            return Some(Self::Progress(progress));
        }
        None
    }
}
