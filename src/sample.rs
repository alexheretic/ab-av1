//! ffmpeg logic
use crate::{process::ensure_success, temporary, SAMPLE_SIZE_S};
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

/// Create a 20s sample from `sample_start`.
///
/// Fast as this uses `-c:v copy`.
pub async fn copy(input: &Path, sample_start: Duration) -> anyhow::Result<PathBuf> {
    let ext = input
        .extension()
        .context("input has no extension")?
        .to_string_lossy();
    let dest = input.with_extension(format!(
        "sample{}+{SAMPLE_SIZE_S}.{ext}",
        sample_start.as_secs()
    ));

    temporary::add(&dest);

    let out = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-ss")
        .arg(sample_start.as_secs().to_string())
        .arg("-t")
        .arg(SAMPLE_SIZE_S.to_string())
        .arg("-c:v")
        .arg("copy")
        .arg("-an")
        .arg(&dest)
        .output()
        .await
        .context("ffmpeg copy")?;

    ensure_success("ffmpeg copy", &out)?;
    Ok(dest)
}
