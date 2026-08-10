//! Optional verification of a finished encode.
//!
//! ffmpeg can exit successfully having written a damaged or short result, usually
//! because the input itself is damaged. Neither the exit code nor sample based VMAF
//! notices this, since the samples may all land on healthy parts of the file.
//! Verification decodes the finished encode and checks it against the input.
use crate::process::{Chunks, CommandExt, FfmpegOut, exit_ok_stderr};
use anyhow::{Context, ensure};
use log::{debug, info};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::StreamExt;

/// Allowed difference between the input duration & the output duration.
///
/// Encoders may round the final frame duration, so an exact match can't be required.
const DURATION_TOLERANCE: Duration = Duration::from_secs(2);

/// Decode `output` in full and check it against the input.
///
/// `expect_duration` is the duration the output should have, if that can be known.
/// `on_progress` is called with the decode progress, so callers can keep a progress
/// bar moving during what is a full pass over the output.
pub async fn verify(
    output: &Path,
    expect_duration: Option<Duration>,
    expect_audio: bool,
    on_progress: impl FnMut(FfmpegOut),
) -> anyhow::Result<()> {
    decode(output, on_progress).await?;
    let probe = crate::ffprobe::probe(output);

    if let Some(expected) = expect_duration
        && let Ok(actual) = &probe.duration
        // zero means the output declares no duration, e.g. raw stream outputs
        && !actual.is_zero()
    {
        ensure!(
            expected.abs_diff(*actual) <= DURATION_TOLERANCE,
            "verify: output is {} long, input is {} long. \
             The input is likely truncated or damaged",
            humantime::format_duration(round_ms(*actual)),
            humantime::format_duration(round_ms(expected)),
        );
    }

    if expect_audio {
        ensure!(
            probe.has_audio,
            "verify: input has audio but the output does not"
        );
    }

    Ok(())
}

/// Decode all video & audio streams of `file`, failing on the first decode error.
async fn decode(file: &Path, mut on_progress: impl FnMut(FfmpegOut)) -> anyhow::Result<()> {
    info!(
        "verifying {}",
        file.file_name().and_then(|n| n.to_str()).unwrap_or("")
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.kill_on_drop(true)
        // Only errors are of interest, `-stats` keeps the progress output that the
        // "error" log level would otherwise hide.
        .arg2("-v", "error")
        .arg("-stats")
        .arg("-xerror")
        .arg2("-i", file)
        // Decode all video & audio streams, not just the ones ffmpeg would select by
        // default. Subtitles, attachments & data are skipped as the null muxer has no
        // encoder for them, and the encode copies them through unchanged anyway.
        .arg2("-map", "0:v?")
        .arg2("-map", "0:a?")
        .arg2("-f", "null")
        .arg("-")
        .stdin(Stdio::null());

    let cmd_str = cmd.to_cmd_str();
    debug!("cmd `{cmd_str}`");
    let mut dec = crate::process::child::AddOnDropChunkStream::from(
        ProcessChunkStream::try_from(cmd).context("ffmpeg verify")?,
    );

    let mut chunks = Chunks::default();
    while let Some(next) = dec.next().await {
        match next {
            Item::Stderr(chunk) => {
                chunks.push(&chunk);
                if let Some(out) = FfmpegOut::try_parse(chunks.last_line()) {
                    on_progress(out);
                }
            }
            Item::Stdout(_) => {}
            Item::Done(code) => exit_ok_stderr("ffmpeg verify", code, &cmd_str, &chunks)?,
        }
    }

    Ok(())
}

/// Drop sub-millisecond parts so durations print readably.
fn round_ms(duration: Duration) -> Duration {
    Duration::from_millis(duration.as_millis().try_into().unwrap_or(u64::MAX))
}
