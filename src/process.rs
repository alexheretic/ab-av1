use anyhow::ensure;
use std::{
    ffi::OsStr,
    io,
    process::{ExitStatus, Output},
    time::Duration,
};
use time::macros::format_description;
use tokio::process::Child;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

pub fn ensure_success(name: &'static str, out: &Output) -> anyhow::Result<()> {
    ensure!(
        out.status.success(),
        "{name} exit code {:?}\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    Ok(())
}

/// Convert exit code result into simple result.
pub fn exit_ok(name: &'static str, done: io::Result<ExitStatus>) -> anyhow::Result<()> {
    let code = done?;
    ensure!(code.success(), "{name} exit code {:?}", code.code());
    Ok(())
}

/// Ok -> None, err -> Some(err)
pub fn exit_ok_option<T>(
    name: &'static str,
    done: io::Result<ExitStatus>,
) -> Option<anyhow::Result<T>> {
    match exit_ok(name, done) {
        Ok(_) => None,
        Err(err) => Some(Err(err)),
    }
}

#[derive(Debug, PartialEq)]
pub struct FfmpegProgress {
    pub frame: u64,
    pub fps: f32,
    pub time: Duration,
}

impl FfmpegProgress {
    pub fn try_parse(out: &str) -> Option<Self> {
        if out.starts_with("frame=") && out.ends_with('\r') {
            let frame: u64 = parse_label_substr("frame=", out)?.parse().ok()?;
            let fps: f32 = parse_label_substr("fps=", out)?.parse().ok()?;
            let (h, m, s, ns) = time::Time::parse(
                parse_label_substr("time=", out)?,
                &format_description!("[hour]:[minute]:[second].[subsecond]"),
            )
            .unwrap()
            .as_hms_nano();
            return Some(Self {
                frame,
                fps,
                time: Duration::new(h as u64 * 60 * 60 + m as u64 * 60 + s as u64, ns),
            });
        }
        None
    }

    pub fn stream(
        child: Child,
        name: &'static str,
    ) -> impl Stream<Item = anyhow::Result<FfmpegProgress>> {
        ProcessChunkStream::from(child).filter_map(move |item| match item {
            Item::Stderr(chunk) => {
                FfmpegProgress::try_parse(&String::from_utf8_lossy(&chunk)).map(Ok)
            }
            Item::Stdout(_) => None,
            Item::Done(code) => exit_ok_option(name, code),
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

#[test]
fn parse_ffmpeg_out() {
    let out = "frame=  288 fps= 94 q=-0.0 size=N/A time=01:23:12.34 bitrate=N/A speed=3.94x    \r";
    assert_eq!(
        FfmpegProgress::try_parse(out),
        Some(FfmpegProgress {
            frame: 288,
            fps: 94.0,
            time: Duration::new(60 * 60 + 23 * 60 + 12, 340_000_000),
        })
    );
}

pub trait CommandExt {
    /// Adds an argument if `condition` otherwise noop.
    fn arg_if(&mut self, condition: bool, a: impl AsRef<OsStr>) -> &mut Self;

    /// Adds two arguments.
    fn arg2(&mut self, a: impl AsRef<OsStr>, b: impl AsRef<OsStr>) -> &mut Self;

    /// Adds two arguments, the 2nd an option. `None` mean noop.
    fn arg2_opt(&mut self, a: impl AsRef<OsStr>, b: Option<impl AsRef<OsStr>>) -> &mut Self;

    /// Adds two arguments if `condition` otherwise noop.
    fn arg2_if(&mut self, condition: bool, a: impl AsRef<OsStr>, b: impl AsRef<OsStr>)
        -> &mut Self;
}
impl CommandExt for tokio::process::Command {
    fn arg_if(&mut self, c: bool, arg: impl AsRef<OsStr>) -> &mut Self {
        match c {
            true => self.arg(arg),
            false => self,
        }
    }

    fn arg2(&mut self, a: impl AsRef<OsStr>, b: impl AsRef<OsStr>) -> &mut Self {
        self.arg(a).arg(b)
    }

    fn arg2_opt(&mut self, a: impl AsRef<OsStr>, b: Option<impl AsRef<OsStr>>) -> &mut Self {
        match b {
            Some(b) => self.arg2(a, b),
            None => self,
        }
    }

    fn arg2_if(&mut self, c: bool, a: impl AsRef<OsStr>, b: impl AsRef<OsStr>) -> &mut Self {
        match c {
            true => self.arg2(a, b),
            false => self,
        }
    }
}
