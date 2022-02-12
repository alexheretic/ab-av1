//! Shared argument logic.
mod svt;

pub use svt::*;

use clap::Parser;
use std::{path::PathBuf, sync::Arc};

/// Encoding args that apply when encoding to an output.
#[derive(Parser, Clone)]
pub struct EncodeToOutput {
    /// Output file, by default the same as input with `.av1` before the extension.
    ///
    /// E.g. if unspecified: -i vid.mp4 --> vid.av1.mp4
    #[clap(short, long)]
    pub output: Option<PathBuf>,

    /// Set the output ffmpeg audio codec. See https://ffmpeg.org/ffmpeg.html#Audio-Options.
    ///
    /// By default when the input & output file extension match 'copy' is used, otherwise
    /// libopus is used.
    #[clap(long = "acodec")]
    pub audio_codec: Option<String>,
}

/// Common vmaf options.
#[derive(Parser, Clone)]
pub struct Vmaf {
    /// Additional vmaf arg(s). E.g. --vmaf n_threads=8 --vmaf n_subsample=4
    ///
    /// See https://ffmpeg.org/ffmpeg-filters.html#libvmaf.
    #[clap(long = "vmaf", parse(try_from_str = parse_vmaf_arg))]
    pub args: Vec<Arc<str>>,
}

fn parse_vmaf_arg(arg: &str) -> anyhow::Result<Arc<str>> {
    Ok(arg.to_owned().into())
}

impl Vmaf {
    pub fn ffmpeg_lavfi(&self) -> String {
        if self.args.is_empty() {
            return "libvmaf".into();
        }
        let mut combined = self.args.clone().join(":");
        combined.insert_str(0, "libvmaf=");
        combined
    }
}

#[test]
fn vmaf_lavfi() {
    let vmaf = Vmaf {
        args: vec!["n_threads=5".into(), "n_subsample=4".into()],
    };
    assert_eq!(vmaf.ffmpeg_lavfi(), "libvmaf=n_threads=5:n_subsample=4");
}

#[test]
fn vmaf_lavfi_default() {
    let vmaf = Vmaf { args: vec![] };
    assert_eq!(vmaf.ffmpeg_lavfi(), "libvmaf");
}
