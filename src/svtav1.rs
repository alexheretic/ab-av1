//! svt-av1 logic
use crate::temporary::TemporaryPath;
use anyhow::{ensure, Context};
use std::{path::Path, process::Stdio};
use tokio::process::Command;

pub async fn encode(sample: &Path, crf: u8, preset: u8) -> anyhow::Result<TemporaryPath> {
    let dest = sample.with_extension(format!("crf{crf}.p{preset}.ivf"));

    let mut yuv4mpegpipe = Command::new("ffmpeg")
        .kill_on_drop(true)
        .arg("-i")
        .arg(sample)
        .arg("-strict")
        .arg("-1")
        .arg("-f")
        .arg("yuv4mpegpipe")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("ffmpeg yuv4mpegpipe")?;

    let yuv4mpegpipe_out: Stdio = yuv4mpegpipe.stdout.take().unwrap().try_into().unwrap();

    let out = Command::new("SvtAv1EncApp")
        .arg("-i")
        .arg("stdin")
        .arg("--crf")
        .arg(crf.to_string())
        .arg("--preset")
        .arg(preset.to_string())
        .arg("-b")
        .arg(&dest)
        .stdin(yuv4mpegpipe_out)
        .output()
        .await
        .context("SvtAv1EncApp")?;

    ensure!(
        out.status.success(),
        "SvtAv1EncApp: {}\n{}\n{}",
        out.status.code().map(|c| c.to_string()).unwrap_or_default(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    Ok(dest.into())
}
