//! ffmpeg logic
use crate::{
    process::{ensure_success, CommandExt},
    temporary::{self, TempKind},
    SAMPLE_SIZE_S,
};
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

/// Create a 20s sample from `sample_start`.
///
/// Fast as this uses `-c:v copy`.
pub async fn copy(
    input: &Path,
    sample_start: Duration,
    temp_dir: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .context("input has no extension")?;
    let mut dest = input.with_extension(format!(
        "sample{}+{SAMPLE_SIZE_S}.{ext}",
        sample_start.as_secs()
    ));
    if let (Some(mut temp), Some(name)) = (temp_dir, dest.file_name()) {
        temp.push(name);
        dest = temp;
    }
    if dest.exists() {
        return Ok(dest);
    }
    temporary::add(&dest, TempKind::Keepable);

    let out = Command::new("ffmpeg")
        .arg("-y")
        .arg2("-ss", sample_start.as_secs().to_string())
        .arg2("-i", input)
        .arg2("-t", SAMPLE_SIZE_S.to_string())
        .arg2("-c:v", "copy")
        .arg("-an")
        .arg(&dest)
        .output()
        .await
        .context("ffmpeg copy")?;

    ensure_success("ffmpeg copy", &out)?;
    Ok(dest)
}
