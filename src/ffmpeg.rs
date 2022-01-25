//! ffmpeg logic
use crate::{temporary::TemporaryPath, SAMPLE_SIZE_S};
use anyhow::{ensure, Context};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::Command;

/// Create a 20s sample from `sample_start`, or re-use if it already exists.
pub async fn cut_sample(input: &Path, sample_start: Duration) -> anyhow::Result<TemporaryPath> {
    let ext = input
        .extension()
        .context("input has no extension")?
        .to_string_lossy();
    let dest = input.with_extension(format!(
        "sample{}+{SAMPLE_SIZE_S}.{ext}",
        sample_start.as_secs()
    ));

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
        .stderr(Stdio::piped())
        .output()
        .await
        .context("ffmpeg cut")?;

    let stderr = String::from_utf8_lossy(&out.stderr);

    ensure!(
        out.status.success(),
        "ffmpeg cut: {}\n{}\n{}",
        out.status.code().map(|c| c.to_string()).unwrap_or_default(),
        String::from_utf8_lossy(&out.stdout),
        stderr,
    );

    Ok(dest.into())
}

/// Calculate the VMAF at 24fps.
pub async fn vmaf(original: &Path, encoded: &Path) -> anyhow::Result<f32> {
    let out = Command::new("ffmpeg")
        .arg("-r")
        .arg("24")
        .arg("-i")
        .arg(encoded)
        .arg("-r")
        .arg("24")
        .arg("-i")
        .arg(original)
        .arg("-lavfi")
        .arg("libvmaf")
        .arg("-f")
        .arg("null")
        .arg("-")
        .output()
        .await
        .context("ffmpeg vmaf")?;

    let stderr = String::from_utf8_lossy(&out.stderr);

    ensure!(
        out.status.success(),
        "ffmpeg vmaf: {}\n{}\n{}",
        out.status.code().map(|c| c.to_string()).unwrap_or_default(),
        String::from_utf8_lossy(&out.stdout),
        stderr,
    );

    let score_idx = stderr.find("VMAF score: ").context("invalid vmaf output")?;
    Ok(stderr[score_idx + "VMAF score: ".len()..].trim().parse()?)
}
