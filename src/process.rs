pub mod child;

use anyhow::{anyhow, ensure};
use std::{
    borrow::Cow,
    ffi::OsStr,
    fmt::Display,
    io,
    pin::Pin,
    process::{ExitStatus, Output},
    sync::Arc,
    task::{Context, Poll, ready},
    time::Duration,
};
use time::macros::format_description;
use tokio::process::Child;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::Stream;

pub fn ensure_success(name: &'static str, out: &Output) -> anyhow::Result<()> {
    ensure!(
        out.status.success(),
        "{name} exit code {}\n---stderr---\n{}\n------------",
        out.status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "None".into()),
        String::from_utf8_lossy(&out.stderr).trim(),
    );
    Ok(())
}

/// Convert exit code result into simple result.
pub fn exit_ok(name: &'static str, done: io::Result<ExitStatus>) -> anyhow::Result<()> {
    let code = done?;
    ensure!(
        code.success(),
        "{name} exit code {}",
        code.code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "None".into())
    );
    Ok(())
}

/// Convert exit code result into simple result adding stderr to error messages.
pub fn exit_ok_stderr(
    name: &'static str,
    done: io::Result<ExitStatus>,
    cmd_str: &str,
    stderr: &Chunks,
) -> anyhow::Result<()> {
    exit_ok(name, done).map_err(|e| cmd_err(e, cmd_str, stderr))
}

pub fn cmd_err(err: impl Display, cmd_str: &str, stderr: &Chunks) -> anyhow::Error {
    anyhow!(
        "{err}\n----cmd-----\n{cmd_str}\n---stderr---\n{}\n------------",
        String::from_utf8_lossy(&stderr.out).trim()
    )
}

#[derive(Debug, PartialEq)]
pub enum FfmpegOut {
    Progress {
        frame: u64,
        fps: f32,
        time: Duration,
    },
    StreamSizes {
        video: u64,
        audio: u64,
        subtitle: u64,
        other: u64,
    },
}

impl FfmpegOut {
    pub fn try_parse(line: &str) -> Option<Self> {
        if line.starts_with("frame=") {
            let frame: u64 = parse_label_substr("frame=", line)?.parse().ok()?;
            let fps: f32 = parse_label_substr("fps=", line)?.parse().ok()?;
            let (h, m, s, ns) = time::Time::parse(
                parse_label_substr("time=", line)?,
                &format_description!("[hour]:[minute]:[second].[subsecond]"),
            )
            .ok()?
            .as_hms_nano();
            return Some(Self::Progress {
                frame,
                fps,
                time: Duration::new(h as u64 * 60 * 60 + m as u64 * 60 + s as u64, ns),
            });
        }
        if line.starts_with("video:") && line.contains("muxing overhead") {
            let video = parse_label_size("video:", line)?;
            let audio = parse_label_size("audio:", line)?;
            let subtitle = parse_label_size("subtitle:", line)?;
            let other = parse_label_size("other streams:", line)?;
            return Some(Self::StreamSizes {
                video,
                audio,
                subtitle,
                other,
            });
        }
        None
    }

    pub fn stream(child: Child, name: &'static str, cmd_str: String) -> FfmpegOutStream {
        FfmpegOutStream {
            chunk_stream: ProcessChunkStream::from(child),
            chunks: <_>::default(),
            name,
            cmd_str,
        }
    }
}

/// Parse a ffmpeg `label=  value ` type substring.
fn parse_label_substr<'a>(label: &str, line: &'a str) -> Option<&'a str> {
    let line = &line[line.find(label)? + label.len()..];
    let val_start = line.char_indices().find(|(_, c)| !c.is_whitespace())?.0;
    let val_end = val_start
        + line[val_start..]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| line[val_start..].len());

    Some(&line[val_start..val_end])
}

fn parse_label_size(label: &str, line: &str) -> Option<u64> {
    let size = parse_label_substr(label, line)?;
    let kbs: u64 = size.strip_suffix("kB")?.parse().ok()?;
    Some(kbs * 1024)
}

/// Output chunk storage.
///
/// Stores up to ~32k chunk data on the heap.
#[derive(Default)]
pub struct Chunks {
    out: Vec<u8>,
    /// Truncate to this index before the next Self::push
    trunc_next_push: Option<usize>,
}

impl Chunks {
    /// Append a chunk.
    ///
    /// If the chunk **ends** in a '\r' carriage returns this will trigger
    /// appropriate overwriting on the next call to `push`.
    ///
    /// Removes oldest lines if storage exceeds maximum.
    pub fn push(&mut self, chunk: &[u8]) {
        const MAX_LEN: usize = 32_000;

        if let Some(idx) = self.trunc_next_push.take() {
            self.out.truncate(idx);
        }

        self.out.extend(chunk);

        // if too long remove lines until small
        while self.out.len() > MAX_LEN {
            self.rm_oldest_line();
        }

        // Setup `trunc_next_push` driven by '\r'
        // Typically progress updates, e.g. ffmpeg:
        // ```text
        // frame=  495 fps= 25 q=40.0 size=     768KiB time=00:00:16.47 bitrate= 381.8kbits/s speed=0.844x    \r
        // ```
        if chunk.ends_with(b"\r") {
            self.trunc_next_push = Some(self.after_last_line_feed());
        }
    }

    /// Returns index after the latest '\n' or 0 if there are none.
    fn after_last_line_feed(&self) -> usize {
        self.out
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(0)
    }

    fn rm_oldest_line(&mut self) {
        let mut next_eol = self
            .out
            .iter()
            .position(|b| *b == b'\n')
            .unwrap_or(self.out.len() - 1);
        if self.out.get(next_eol + 1) == Some(&b'\r') {
            next_eol += 1;
        }

        self.out.splice(..next_eol + 1, []);
    }

    pub fn rfind_line(&self, predicate: impl Fn(&str) -> bool) -> Option<&str> {
        self.rfind_line_map(|line| predicate(line).then_some(line))
    }

    pub fn rfind_line_map<'a, T>(&'a self, f: impl Fn(&'a str) -> Option<T>) -> Option<T> {
        let lines = self
            .out
            .rsplit(|b| *b == b'\n')
            .flat_map(|l| l.rsplit(|b| *b == b'\r'));
        for line in lines {
            if let Ok(line) = std::str::from_utf8(line)
                && let Some(out) = f(line)
            {
                return Some(out);
            }
        }
        None
    }

    /// Returns last non-empty line, if any.
    pub fn last_line(&self) -> &str {
        self.rfind_line(|l| !l.is_empty()).unwrap_or_default()
    }
}

pin_project_lite::pin_project! {
    #[must_use = "streams do nothing unless polled"]
    pub struct FfmpegOutStream {
        #[pin]
        chunk_stream: ProcessChunkStream,
        name: &'static str,
        cmd_str: String,
        chunks: Chunks,
    }
}

impl FfmpegOutStream {
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        match self.chunk_stream.child_mut() {
            Some(c) => c.wait().await,
            None => Ok(<_>::default()),
        }
    }
}

impl Stream for FfmpegOutStream {
    type Item = anyhow::Result<FfmpegOut>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.as_mut().project().chunk_stream.poll_next(cx)) {
                Some(item) => match item {
                    Item::Stderr(chunk) => {
                        self.chunks.push(&chunk);
                        if let Some(out) = FfmpegOut::try_parse(self.chunks.last_line()) {
                            return Poll::Ready(Some(Ok(out)));
                        }
                    }
                    Item::Stdout(_) => {}
                    Item::Done(code) => {
                        if let Err(err) =
                            exit_ok_stderr(self.name, code, &self.cmd_str, &self.chunks)
                        {
                            return Poll::Ready(Some(Err(err)));
                        }
                    }
                },
                None => return Poll::Ready(None),
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.chunk_stream.size_hint().1)
    }
}

#[test]
fn parse_ffmpeg_progress_chunk() {
    let out = "frame=  288 fps= 94 q=-0.0 size=N/A time=01:23:12.34 bitrate=N/A speed=3.94x    \r";
    assert_eq!(
        FfmpegOut::try_parse(out),
        Some(FfmpegOut::Progress {
            frame: 288,
            fps: 94.0,
            time: Duration::new(60 * 60 + 23 * 60 + 12, 340_000_000),
        })
    );
}

#[test]
fn parse_ffmpeg_progress_line() {
    let out = "frame=  161 fps= 73 q=-0.0 size=  978076kB time=00:00:06.71 bitrate=1193201.6kbits/s dup=13 drop=0 speed=3.03x    ";
    assert_eq!(
        FfmpegOut::try_parse(out),
        Some(FfmpegOut::Progress {
            frame: 161,
            fps: 73.0,
            time: Duration::new(6, 710_000_000),
        })
    );
}

#[test]
fn parse_ffmpeg_progress_na_time() {
    let out = "frame=  288 fps= 94 q=-0.0 size=N/A time=N/A bitrate=N/A speed=3.94x    ";
    assert_eq!(FfmpegOut::try_parse(out), None);
}

#[test]
fn parse_ffmpeg_stream_sizes() {
    let out = "video:2897022kB audio:537162kB subtitle:0kB other streams:0kB global headers:0kB muxing overhead: 0.289700%\n";
    assert_eq!(
        FfmpegOut::try_parse(out),
        Some(FfmpegOut::StreamSizes {
            video: 2897022 * 1024,
            audio: 537162 * 1024,
            subtitle: 0,
            other: 0,
        })
    );
}

pub trait CommandExt {
    /// Adds two arguments.
    fn arg2(&mut self, a: impl ArgString, b: impl ArgString) -> &mut Self;

    /// Adds two arguments, the 2nd an option. `None` mean noop.
    fn arg2_opt(&mut self, a: impl ArgString, b: Option<impl ArgString>) -> &mut Self;

    /// Adds two arguments if `condition` otherwise noop.
    fn arg2_if(&mut self, condition: bool, a: impl ArgString, b: impl ArgString) -> &mut Self;

    /// Adds an argument if `condition` otherwise noop.
    fn arg_if(&mut self, condition: bool, a: impl ArgString) -> &mut Self;

    /// Convert to readable shell-like string.
    fn to_cmd_str(&self) -> String;
}
impl CommandExt for tokio::process::Command {
    fn arg2(&mut self, a: impl ArgString, b: impl ArgString) -> &mut Self {
        self.arg(a.arg_string()).arg(b.arg_string())
    }

    fn arg2_opt(&mut self, a: impl ArgString, b: Option<impl ArgString>) -> &mut Self {
        match b {
            Some(b) => self.arg2(a, b),
            None => self,
        }
    }

    fn arg2_if(&mut self, c: bool, a: impl ArgString, b: impl ArgString) -> &mut Self {
        match c {
            true => self.arg2(a, b),
            false => self,
        }
    }

    fn arg_if(&mut self, condition: bool, a: impl ArgString) -> &mut Self {
        match condition {
            true => self.arg(a.arg_string()),
            false => self,
        }
    }

    fn to_cmd_str(&self) -> String {
        let cmd = self.as_std();
        cmd.get_args().map(|a| a.to_string_lossy()).fold(
            cmd.get_program().to_string_lossy().to_string(),
            |mut all, next| {
                all.push(' ');
                all += &next;
                all
            },
        )
    }
}

pub trait ArgString {
    fn arg_string(&self) -> Cow<'_, OsStr>;
}

macro_rules! impl_arg_string_as_ref {
    ($t:ty) => {
        impl ArgString for $t {
            fn arg_string(&self) -> Cow<'_, OsStr> {
                Cow::Borrowed(self.as_ref())
            }
        }
    };
}
impl_arg_string_as_ref!(String);
impl_arg_string_as_ref!(&'_ String);
impl_arg_string_as_ref!(&'_ str);
impl_arg_string_as_ref!(&'_ &'_ str);
impl_arg_string_as_ref!(&'_ std::path::Path);
impl_arg_string_as_ref!(&'_ std::path::PathBuf);

macro_rules! impl_arg_string_display {
    ($t:ty) => {
        impl ArgString for $t {
            fn arg_string(&self) -> Cow<'_, OsStr> {
                Cow::Owned(self.to_string().into())
            }
        }
    };
}
impl_arg_string_display!(u8);
impl_arg_string_display!(u16);
impl_arg_string_display!(u32);
impl_arg_string_display!(i32);
impl_arg_string_display!(f32);

impl ArgString for Arc<str> {
    fn arg_string(&self) -> Cow<'_, OsStr> {
        Cow::Borrowed((**self).as_ref())
    }
}
