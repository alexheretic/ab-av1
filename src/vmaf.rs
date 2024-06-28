//! vmaf logic
use crate::process::{cmd_err, exit_ok_stderr, Chunks, CommandExt, FfmpegOut};
use anyhow::Context;
use log::{debug, info};
use std::path::Path;
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Calculate VMAF score by converting the original first to yuv.
/// This can produce more accurate results than testing directly from original source.
pub fn run(
    reference: &Path,
    distorted: &Path,
    filter_complex: &str,
) -> anyhow::Result<impl Stream<Item = VmafOut>> {
    info!(
        "vmaf {} vs reference {}",
        distorted.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        reference.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.kill_on_drop(true)
        .arg2("-r", "24")
        .arg2("-i", distorted)
        .arg2("-r", "24")
        .arg2("-i", reference)
        .arg2("-filter_complex", filter_complex)
        .arg2("-f", "null")
        .arg("-");

    let cmd_str = cmd.to_cmd_str();
    debug!("cmd `{cmd_str}`");
    let vmaf: ProcessChunkStream = cmd.try_into().context("ffmpeg vmaf")?;

    Ok(async_stream::stream! {
        let mut vmaf = vmaf;
        let mut chunks = Chunks::default();
        let mut parsed_done = false;
        let mut exit_ok = false;
        while let Some(next) = vmaf.next().await {
            match next {
                Item::Stderr(chunk) => {
                    if let Some(out) = VmafOut::try_from_chunk(&chunk, &mut chunks) {
                        if matches!(out, VmafOut::Done(_)) {
                            parsed_done = true;
                        }
                        yield out;
                    }
                }
                Item::Stdout(_) => {}
                Item::Done(code) => match exit_ok_stderr("ffmpeg vmaf", code, &cmd_str, &chunks) {
                    Ok(_) => exit_ok = true,
                    Err(err) => yield VmafOut::Err(err),
                },
            }
        }
        if exit_ok && !parsed_done {
            yield VmafOut::Err(cmd_err(
                "could not parse ffmpeg vmaf score",
                &cmd_str,
                &chunks,
            ));
        }
    })
}

#[derive(Debug)]
pub enum VmafOut {
    Progress(FfmpegOut),
    Done(f32),
    Err(anyhow::Error),
}

impl VmafOut {
    fn try_from_chunk(chunk: &[u8], chunks: &mut Chunks) -> Option<Self> {
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
