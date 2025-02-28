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

/// Create a sample from `sample_start` + `frames`.
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

    let mut dest = temporary::process_dir(temp_dir);
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
