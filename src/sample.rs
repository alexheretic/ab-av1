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

/// The file name used for a copy sample of `input` at `sample_start` with `frames`.
///
/// Mirrors the destination file name used by [`copy`], so the sample-encode cache key
/// derived from the file name can be computed without creating the file.
pub fn sample_file_name(
    input: &Path,
    sample_start: Duration,
    floor_to_sec: bool,
    frames: u32,
) -> String {
    let mut sample_start_s = sample_start.as_secs_f32();
    if floor_to_sec {
        sample_start_s = sample_start_s.floor();
    }
    input
        .with_extension(format!("sample{sample_start_s}+{frames}f.mkv"))
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

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

    let mut dest = temporary::process_dir(temp_dir)?;
    // Always using mkv for the samples works better than, e.g. using mp4 for mp4s
    // see https://github.com/alexheretic/ab-av1/issues/82#issuecomment-1337306325
    dest.push(sample_file_name(input, sample_start, floor_to_sec, frames));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_file_name_uses_mkv_and_floor_when_floor_to_sec() {
        let input = PathBuf::from("/tmp/movie.mkv");
        let start = Duration::from_millis(15200);
        let name = sample_file_name(&input, start, true, 120);
        assert_eq!(name, "movie.sample15+120f.mkv");
    }

    #[test]
    fn sample_file_name_keeps_fraction_when_not_floored() {
        let input = PathBuf::from("/tmp/movie.mkv");
        let start = Duration::from_millis(15200);
        let name = sample_file_name(&input, start, false, 120);
        assert_eq!(name, "movie.sample15.2+120f.mkv");
    }
}
