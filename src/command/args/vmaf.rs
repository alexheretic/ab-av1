use anyhow::Context;
use clap::Parser;
use std::{fmt::Display, sync::Arc};

/// Common vmaf options.
#[derive(Parser, Clone)]
pub struct Vmaf {
    /// Additional vmaf arg(s). E.g. --vmaf n_threads=8 --vmaf n_subsample=4
    ///
    /// See https://ffmpeg.org/ffmpeg-filters.html#libvmaf.
    #[clap(long = "vmaf", parse(from_str = parse_vmaf_arg))]
    pub vmaf_args: Vec<Arc<str>>,

    /// Video resolution scale to use in VMAF analysis. If set, video streams will be bicupic
    /// scaled to this width during VMAF analysis. `auto` (default) automatically sets
    /// based on the model and input video resolution. `none` disables any scaling.
    /// `WxH` format may be used to specify custom scaling, e.g. `1920x1080`.
    ///
    /// Default automatic behaviour:
    /// * w < 1728 and h < 972 => scale to 1080p (without changing aspect)
    /// * w >= 1728 or h >= 972 => no scaling
    ///
    /// Scaling happens after any input/reference vfilters.
    #[clap(long, default_value_t = VmafScale::Auto, parse(try_from_str = parse_vmaf_scale))]
    pub vmaf_scale: VmafScale,
}

fn parse_vmaf_arg(arg: &str) -> Arc<str> {
    arg.to_owned().into()
}

impl Vmaf {
    /// Returns ffmpeg `filter_complex`/`lavfi` value for calculating vmaf.
    pub fn ffmpeg_lavfi(&self, distorted_res: Option<(u32, u32)>) -> String {
        let mut args = self.vmaf_args.clone();
        if !args.iter().any(|a| a.contains("n_threads")) {
            // default n_threads to all cores
            args.push(format!("n_threads={}", num_cpus::get()).into());
        }
        let mut lavfi = args.join(":");
        lavfi.insert_str(0, "libvmaf=");

        if let Some((w, h)) = self.vf_scale(&args, distorted_res) {
            // scale both streams to the vmaf width
            lavfi.insert_str(
                0,
                &format!("[0:v]scale={w}:{h}:flags=bicubic[dis];[1:v]scale={w}:{h}:flags=bicubic[ref];[dis][ref]"),
            );
        }

        lavfi
    }

    fn vf_scale(&self, args: &[Arc<str>], distorted_res: Option<(u32, u32)>) -> Option<(i32, i32)> {
        match (self.vmaf_scale, distorted_res) {
            (VmafScale::Auto, Some((w, h))) => match VmafModel::from_args(args) {
                // upscale small resolutions to 1k for use with the 1k model
                VmafModel::Vmaf1K if w < 1728 && h < 972 => {
                    Some(minimally_scale((w, h), (1920, 1080)))
                }
                _ => None,
            },
            (VmafScale::Custom { width, height }, Some((w, h))) => {
                Some(minimally_scale((w, h), (width, height)))
            }
            (VmafScale::Custom { width, height }, None) => Some((width as _, height as _)),
            _ => None,
        }
    }
}

/// Return the smallest ffmpeg vf `(w, h)` scale values so that at least one of the
/// `target_w` or `target_h` bounds are met.
fn minimally_scale((from_w, from_h): (u32, u32), (target_w, target_h): (u32, u32)) -> (i32, i32) {
    let w_factor = from_w as f64 / target_w as f64;
    let h_factor = from_h as f64 / target_h as f64;
    if h_factor > w_factor {
        (-1, target_h as _) // scale vertically
    } else {
        (target_w as _, -1) // scale horizontally
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VmafScale {
    None,
    Auto,
    Custom { width: u32, height: u32 },
}

fn parse_vmaf_scale(vs: &str) -> anyhow::Result<VmafScale> {
    const ERR: &str = "vmaf-scale must be 'none', 'auto' or WxH format e.g. '1920x1080'";
    match vs {
        "none" => Ok(VmafScale::None),
        "auto" => Ok(VmafScale::Auto),
        _ => {
            let (w, h) = vs.split_once('x').context(ERR)?;
            let (width, height) = (w.parse().context(ERR)?, h.parse().context(ERR)?);
            Ok(VmafScale::Custom { width, height })
        }
    }
}

impl Display for VmafScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => "none".fmt(f),
            Self::Auto => "auto".fmt(f),
            Self::Custom { width, height } => write!(f, "{width}x{height}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum VmafModel {
    /// Default 1080p model.
    Vmaf1K,
    /// Some other user specified model.
    Custom,
}

impl VmafModel {
    fn from_args(args: &[Arc<str>]) -> Self {
        let using_custom_model = args
            .iter()
            .filter(|v| !v.ends_with("version=vmaf_v0.6.1"))
            .any(|v| v.contains("model"));
        match using_custom_model {
            true => Self::Custom,
            false => Self::Vmaf1K,
        }
    }
}

#[test]
fn vmaf_lavfi() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(vmaf.ffmpeg_lavfi(None), "libvmaf=n_threads=5:n_subsample=4");
}

#[test]
fn vmaf_lavfi_default() {
    let vmaf = Vmaf {
        vmaf_args: vec![],
        vmaf_scale: VmafScale::Auto,
    };
    let expected = format!("libvmaf=n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(None), expected);
}

#[test]
fn vmaf_lavfi_include_n_threads() {
    let vmaf = Vmaf {
        vmaf_args: vec!["log_path=output.xml".into()],
        vmaf_scale: VmafScale::Auto,
    };
    let expected = format!("libvmaf=log_path=output.xml:n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(None), expected);
}

/// Low resolution videos should be upscaled to 1080p
#[test]
fn vmaf_lavfi_small_width() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some((1280, 720))),
        "[0:v]scale=1920:-1:flags=bicubic[dis];\
         [1:v]scale=1920:-1:flags=bicubic[ref];\
         [dis][ref]libvmaf=n_threads=5:n_subsample=4"
    );
}

/// If user has overriden the model, don't default a vmaf width
#[test]
fn vmaf_lavfi_small_width_custom_model() {
    let vmaf = Vmaf {
        vmaf_args: vec![
            "model=version=foo".into(),
            "n_threads=5".into(),
            "n_subsample=4".into(),
        ],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some((1280, 720))),
        "libvmaf=model=version=foo:n_threads=5:n_subsample=4"
    );
}

#[test]
fn vmaf_lavfi_custom_model_and_width() {
    let vmaf = Vmaf {
        vmaf_args: vec![
            "model=version=foo".into(),
            "n_threads=5".into(),
            "n_subsample=4".into(),
        ],
        // if specified just do it
        vmaf_scale: VmafScale::Custom {
            width: 123,
            height: 720,
        },
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some((1280, 720))),
        "[0:v]scale=123:-1:flags=bicubic[dis];\
        [1:v]scale=123:-1:flags=bicubic[ref];\
        [dis][ref]libvmaf=model=version=foo:n_threads=5:n_subsample=4"
    );
}

#[test]
fn vmaf_lavfi_1080p() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some((1920, 1080))),
        "libvmaf=n_threads=5:n_subsample=4"
    );
}
