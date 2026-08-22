//! ffmpeg logic
use crate::{
    process::{CommandExt, ensure_success},
    temporary::{self, TempKind},
};
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

/// Create a sample of the non-video (audio + subtitle) streams from `sample_start`
/// for `sample_duration`, returning its file size.
///
/// `-c copy` makes this cheap. The size is used to approximate the input's non-video
/// stream size, so the predicted encode size can be based on the video stream size
/// rather than the whole file size.
pub async fn non_video_size(
    input: &Path,
    sample_start: Duration,
    sample_duration: Duration,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<u64> {
    let mut sample_start_s = sample_start.as_secs_f32();
    if sample_duration >= Duration::from_secs(2) {
        sample_start_s = sample_start_s.floor();
    }

    let mut dest = temporary::process_dir(temp_dir)?;
    dest.push(
        input
            .with_extension(format!(
                "non-video-sample{sample_start_s}+{}s.mkv",
                sample_duration.as_secs_f32()
            ))
            .file_name()
            .unwrap(),
    );
    temporary::add(&dest, TempKind::Keepable);

    let out = Command::new("ffmpeg")
        .arg("-y")
        .arg2("-ss", sample_start_s)
        .arg2("-t", sample_duration.as_secs_f32())
        .arg2("-i", input)
        .arg2("-map", "0:a?")
        .arg2("-map", "0:s?")
        .arg2("-c", "copy")
        .arg(&dest)
        .stdin(Stdio::null())
        .output()
        .await
        .context("ffmpeg non-video copy")?;

    ensure_success("ffmpeg non-video copy", &out)?;
    let size = tokio::fs::metadata(&dest).await?.len();
    let _ = tokio::fs::remove_file(&dest).await;
    Ok(size)
}

/// Copy a sample from `sample_start` + `frames`.
///
/// Fast as this uses `-c:v copy`.
pub async fn copy(
    input: &Path,
    sample_start: Duration,
    floor_to_sec: bool,
    frames: u32,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let mut sample_start_s = sample_start.as_secs_f32();
    if floor_to_sec {
        sample_start_s = sample_start_s.floor();
    }

    let mut dest = temporary::process_dir(temp_dir)?;
    // Always using mkv for the samples works better than, e.g. using mp4 for mp4s
    // see https://github.com/alexheretic/ab-av1/issues/82#issuecomment-1337306325
    dest.push(
        input
            .with_extension(format!("sample{sample_start_s}+{frames}f.mkv"))
            .file_name()
            .unwrap(),
    );
    if dest.exists() {
        return Ok(dest);
    }
    temporary::add(&dest, TempKind::Keepable);

    // Note: `-ss` before `-i` & `-frames:v` instead of `-t`
    // See https://github.com/alexheretic/ab-av1/issues/36#issuecomment-1146634936
    let mut out = Command::new("ffmpeg")
        .arg("-y")
        .arg2("-ss", sample_start_s)
        .arg2("-i", input)
        .arg2("-frames:v", frames)
        .arg2("-c:v", "copy")
        .arg("-an")
        .arg("-sn")
        .arg(&dest)
        .stdin(Stdio::null())
        .output()
        .await
        .context("ffmpeg copy")?;

    if !out.status.success()
        && String::from_utf8_lossy(&out.stderr)
            .contains("Can't write packet with unknown timestamp")
    {
        out = Command::new("ffmpeg")
            .arg("-y")
            // try +genpts workaround
            .arg2("-fflags", "+genpts")
            .arg2("-ss", sample_start_s)
            .arg2("-i", input)
            .arg2("-frames:v", frames)
            .arg2("-c:v", "copy")
            .arg("-an")
            .arg("-sn")
            .arg(&dest)
            .stdin(Stdio::null())
            .output()
            .await
            .context("ffmpeg copy")?;
    }

    ensure_success("ffmpeg copy", &out)?;
    Ok(dest)
}
