use anyhow::{anyhow, ensure};
use std::{
    borrow::Cow,
    ffi::OsStr,
    io,
    process::{ExitStatus, Output},
    sync::Arc,
    time::Duration,
};
use time::macros::format_description;
use tokio::process::Child;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

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
    stderr: &Chunks,
) -> anyhow::Result<()> {
    exit_ok(name, done)
        .map_err(|e| anyhow!("{e}\n---stderr---\n{}\n------------", stderr.out.trim()))
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

    pub fn stream(
        child: Child,
        name: &'static str,
    ) -> impl Stream<Item = anyhow::Result<FfmpegOut>> {
        let mut chunks = Chunks::default();
        ProcessChunkStream::from(child).filter_map(move |item| match item {
            Item::Stderr(chunk) => {
                chunks.push(&chunk);
                FfmpegOut::try_parse(chunks.last_line()).map(Ok)
            }
            Item::Stdout(_) => None,
            Item::Done(code) => match exit_ok_stderr(name, code, &chunks) {
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            },
        })
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
/// Stores up to ~4k chunk data on the heap.
#[derive(Default)]
pub struct Chunks {
    out: String,
}

impl Chunks {
    /// Append a chunk.
    pub fn push(&mut self, chunk: &[u8]) {
        const MAX_LEN: usize = 4000;

        self.out.push_str(&String::from_utf8_lossy(chunk));

        // truncate beginning if too long
        let len = self.out.len();
        if len > MAX_LEN + 100 {
            self.out = String::from_utf8_lossy(&self.out.as_bytes()[len - MAX_LEN..]).into();
        }
    }

    fn rlines(&self) -> impl Iterator<Item = &'_ str> {
        self.out
            .rsplit_terminator('\n')
            .flat_map(|l| l.rsplit_terminator('\r'))
    }

    /// Returns last non-empty line, if any.
    pub fn last_line(&self) -> &str {
        self.rlines().find(|l| !l.is_empty()).unwrap_or_default()
    }
}

#[test]
fn rlines_rn() {
    let mut chunks = Chunks::default();
    chunks.push(b"something \r fooo    \r\n");
    let mut rlines = chunks.rlines();
    assert_eq!(rlines.next(), Some(" fooo    "));
    assert_eq!(rlines.next(), Some("something "));
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
    /// Adds an argument if `condition` otherwise noop.
    fn arg_if(&mut self, condition: bool, a: impl ArgString) -> &mut Self;

    /// Adds two arguments.
    fn arg2(&mut self, a: impl ArgString, b: impl ArgString) -> &mut Self;

    /// Adds two arguments, the 2nd an option. `None` mean noop.
    fn arg2_opt(&mut self, a: impl ArgString, b: Option<impl ArgString>) -> &mut Self;

    /// Adds two arguments if `condition` otherwise noop.
    fn arg2_if(&mut self, condition: bool, a: impl ArgString, b: impl ArgString) -> &mut Self;
}
impl CommandExt for tokio::process::Command {
    fn arg_if(&mut self, c: bool, arg: impl ArgString) -> &mut Self {
        match c {
            true => self.arg(arg.arg_string()),
            false => self,
        }
    }

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
