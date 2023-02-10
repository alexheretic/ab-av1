//! vmaf logic
use crate::{
    command::args::PixelFormat,
    process::{exit_ok_stderr, Chunks, CommandExt, FfmpegOut},
};
use anyhow::Context;
use std::path::Path;
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
    // convert reference & distorted to yuv streams of the same pixel format
    // frame rate and presentation timestamp to improve vmaf accuracy
    let (yuv_out, yuv_pipe) = yuv::pipe(reference, pix_fmt, reference_vfilter)?;
    let yuv_pipe = yuv_pipe.filter_map(VmafOut::ignore_ok);

    #[cfg(unix)]
    let (distorted_fifo, distorted_yuv_pipe) = yuv::unix::pipe_to_fifo(distorted, pix_fmt)?;
    #[cfg(unix)]
    let (distorted, yuv_pipe) = (
        &distorted_fifo,
        yuv_pipe.merge(distorted_yuv_pipe.filter_map(VmafOut::ignore_ok)),
    );
    #[cfg(windows)]
    let (distorted_npipe, distorted_yuv_pipe) = yuv::windows::named_pipe(distorted, pix_fmt)?;
    #[cfg(windows)]
    let (distorted, yuv_pipe) = (
        &distorted_npipe,
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

    let mut chunks = Chunks::default();
    let vmaf = vmaf.filter_map(move |item| match item {
        Item::Stderr(chunk) => VmafOut::try_from_chunk(&chunk, &mut chunks),
        Item::Stdout(_) => None,
        Item::Done(code) => VmafOut::ignore_ok(exit_ok_stderr("ffmpeg vmaf", code, &chunks)),
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

mod yuv {
    use super::*;
    use std::process::Stdio;

    /// ffmpeg yuv4mpegpipe returning the stdout & [`FfmpegProgress`] stream.
    pub fn pipe(
        input: &Path,
        pix_fmt: PixelFormat,
        vfilter: Option<&str>,
    ) -> anyhow::Result<(Stdio, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
        // sync presentation timestamp
        let vfilter: std::borrow::Cow<'_, str> = match vfilter {
            None => "setpts=PTS-STARTPTS".into(),
            Some(vf) if vf.contains("setpts=") => vf.into(),
            Some(vf) => format!("{vf},setpts=PTS-STARTPTS").into(),
        };

        let mut yuv4mpegpipe = Command::new("ffmpeg")
            .kill_on_drop(true)
            // Use 24fps to match vmaf models
            .arg2("-r", "24")
            .arg2("-i", input)
            .arg2("-pix_fmt", pix_fmt.as_str())
            .arg2("-vf", vfilter.as_ref())
            .arg2("-strict", "-1")
            .arg2("-f", "yuv4mpegpipe")
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("ffmpeg yuv4mpegpipe")?;
        let stdout = yuv4mpegpipe.stdout.take().unwrap().try_into().unwrap();
        let stream = FfmpegOut::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");
        Ok((stdout, stream))
    }

    #[cfg(windows)]
    pub mod windows {
        use super::*;

        pub fn named_pipe(
            input: &Path,
            pix_fmt: PixelFormat,
        ) -> anyhow::Result<(String, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
            use rand::{
                distributions::{Alphanumeric, DistString},
                thread_rng,
            };

            let mut in_name = Alphanumeric.sample_string(&mut thread_rng(), 12);
            in_name.insert_str(0, r"\\.\pipe\ab-av1-in-");

            let in_server = tokio::net::windows::named_pipe::ServerOptions::new()
                .access_outbound(false)
                .first_pipe_instance(true)
                .max_instances(1)
                .create(&in_name)?;

            let out_name = in_name.replacen("-in-", "-out-", 1);
            let out_server = tokio::net::windows::named_pipe::ServerOptions::new()
                .access_inbound(false)
                .first_pipe_instance(true)
                .max_instances(1)
                .create(&out_name)?;

            async fn copy_in_pipe_to_out(
                mut in_pipe: tokio::net::windows::named_pipe::NamedPipeServer,
                mut out_pipe: tokio::net::windows::named_pipe::NamedPipeServer,
            ) -> tokio::io::Result<()> {
                in_pipe.connect().await?;
                in_pipe.readable().await?;
                out_pipe.connect().await?;
                out_pipe.writable().await?;
                tokio::io::copy(&mut in_pipe, &mut out_pipe).await?;
                Ok(())
            }
            tokio::spawn(async move {
                if let Err(err) = copy_in_pipe_to_out(in_server, out_server).await {
                    eprintln!("Error copy_in_pipe_to_out: {err}");
                }
            });

            let yuv4mpegpipe = Command::new("ffmpeg")
                .kill_on_drop(true)
                .arg2("-r", "24")
                .arg2("-i", input)
                .arg2("-pix_fmt", pix_fmt.as_str())
                .arg2("-vf", "setpts=PTS-STARTPTS")
                .arg2("-strict", "-1")
                .arg2("-f", "yuv4mpegpipe")
                .arg("-y")
                .arg(&in_name)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .context("ffmpeg yuv4mpegpipe")?;
            let stream = FfmpegOut::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");

            Ok((out_name, stream))
        }
    }

    #[cfg(unix)]
    pub mod unix {
        use super::*;
        use crate::temporary::{self, TempKind};
        use rand::{
            distributions::{Alphanumeric, DistString},
            thread_rng,
        };
        use std::path::PathBuf;

        /// ffmpeg yuv4mpegpipe returning the temporary fifo path & [`FfmpegProgress`] stream.
        pub fn pipe_to_fifo(
            input: &Path,
            pix_fmt: PixelFormat,
        ) -> anyhow::Result<(PathBuf, impl Stream<Item = anyhow::Result<FfmpegOut>>)> {
            let fifo = PathBuf::from(format!(
                "/tmp/ab-av1-{}.fifo",
                Alphanumeric.sample_string(&mut thread_rng(), 12)
            ));
            unix_named_pipe::create(&fifo, None)?;
            temporary::add(&fifo, TempKind::NotKeepable);

            let yuv4mpegpipe = Command::new("ffmpeg")
                .kill_on_drop(true)
                .arg2("-r", "24")
                .arg2("-i", input)
                .arg2("-pix_fmt", pix_fmt.as_str())
                .arg2("-vf", "setpts=PTS-STARTPTS")
                .arg2("-strict", "-1")
                .arg2("-f", "yuv4mpegpipe")
                .arg("-y")
                .arg(&fifo)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .context("ffmpeg yuv4mpegpipe")?;
            let stream = FfmpegOut::stream(yuv4mpegpipe, "ffmpeg yuv4mpegpipe");
            Ok((fifo, stream))
        }
    }
}
