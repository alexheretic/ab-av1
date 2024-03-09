use crate::command::args::PixelFormat;
use anyhow::Context;
use clap::Parser;
use std::{borrow::Cow, fmt::Display, sync::Arc, thread};

/// Common vmaf options.
#[derive(Parser, Clone, Hash)]
pub struct Vmaf {
    /// Additional vmaf arg(s). E.g. --vmaf n_threads=8 --vmaf n_subsample=4
    ///
    /// By default `n_threads` is set to available system threads.
    ///
    /// Also see https://ffmpeg.org/ffmpeg-filters.html#libvmaf.
    #[arg(long = "vmaf", value_parser = parse_vmaf_arg)]
    pub vmaf_args: Vec<Arc<str>>,

    /// Video resolution scale to use in VMAF analysis. If set, video streams will be bicupic
    /// scaled to this width during VMAF analysis. `auto` (default) automatically sets
    /// based on the model and input video resolution. `none` disables any scaling.
    /// `WxH` format may be used to specify custom scaling, e.g. `1920x1080`.
    ///
    /// auto behaviour:
    /// * 1k model (default for resolutions <= 2560x1440) if width and height
    ///   are less than 1728 & 972 respectively upscale to 1080p. Otherwise no scaling.
    /// * 4k model (default for resolutions > 2560x1440) if width and height
    ///   are less than 3456 & 1944 respectively upscale to 4k. Otherwise no scaling.
    ///
    /// Scaling happens after any input/reference vfilters.
    #[arg(long, default_value_t = VmafScale::Auto, value_parser = parse_vmaf_scale)]
    pub vmaf_scale: VmafScale,
}

fn parse_vmaf_arg(arg: &str) -> anyhow::Result<Arc<str>> {
    Ok(arg.to_owned().into())
}

impl Vmaf {
    pub fn is_default(&self) -> bool {
        self.vmaf_args.is_empty() && self.vmaf_scale == VmafScale::Auto
    }

    /// Returns ffmpeg `filter_complex`/`lavfi` value for calculating vmaf.
    pub fn ffmpeg_lavfi(
        &self,
        distorted_res: Option<(u32, u32)>,
        pix_fmt: PixelFormat,
        ref_vfilter: Option<&str>,
    ) -> String {
        let mut args = self.vmaf_args.clone();
        if !args.iter().any(|a| a.contains("n_threads")) {
            // default n_threads to all cores
            args.push(
                format!(
                    "n_threads={}",
                    thread::available_parallelism().map_or(1, |p| p.get())
                )
                .into(),
            );
        }
        let mut lavfi = args.join(":");
        lavfi.insert_str(0, "libvmaf=");

        let mut model = VmafModel::from_args(&args);
        if let (None, Some((w, h))) = (model, distorted_res) {
            if w > 2560 && h > 1440 {
                // for >2k resolutions use 4k model
                lavfi.push_str(":model=version=vmaf_4k_v0.6.1");
                model = Some(VmafModel::Vmaf4K);
            }
        }

        let ref_vf: Cow<_> = match ref_vfilter {
            None => "".into(),
            Some(vf) if vf.ends_with(',') => vf.into(),
            Some(vf) => format!("{vf},").into(),
        };

        // prefix:
        // * Add reference-vfilter if any
        // * convert both streams to common pixel format
        // * scale to vmaf width if necessary
        // * sync presentation timestamp
        let prefix = if let Some((w, h)) = self.vf_scale(model.unwrap_or_default(), distorted_res) {
            format!(
                "[0:v]format={pix_fmt},scale={w}:{h}:flags=bicubic,setpts=PTS-STARTPTS[dis];\
                 [1:v]format={pix_fmt},{ref_vf}scale={w}:{h}:flags=bicubic,setpts=PTS-STARTPTS[ref];[dis][ref]"
            )
        } else {
            format!(
                "[0:v]format={pix_fmt},setpts=PTS-STARTPTS[dis];\
                 [1:v]format={pix_fmt},{ref_vf}setpts=PTS-STARTPTS[ref];[dis][ref]"
            )
        };

        lavfi.insert_str(0, &prefix);
        lavfi
    }

    fn vf_scale(&self, model: VmafModel, distorted_res: Option<(u32, u32)>) -> Option<(i32, i32)> {
        match (self.vmaf_scale, distorted_res) {
            (VmafScale::Auto, Some((w, h))) => match model {
                // upscale small resolutions to 1k for use with the 1k model
                VmafModel::Vmaf1K if w < 1728 && h < 972 => {
                    Some(minimally_scale((w, h), (1920, 1080)))
                }
                // upscale small resolutions to 4k for use with the 4k model
                VmafModel::Vmaf4K if w < 3456 && h < 1944 => {
                    Some(minimally_scale((w, h), (3840, 2160)))
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
    /// 4k model.
    Vmaf4K,
    /// Some other user specified model.
    Custom,
}

impl Default for VmafModel {
    fn default() -> Self {
        Self::Vmaf1K
    }
}

impl VmafModel {
    fn from_args(args: &[Arc<str>]) -> Option<Self> {
        let mut using_custom_model: Vec<_> = args.iter().filter(|v| v.contains("model")).collect();

        match using_custom_model.len() {
            0 => None,
            1 => Some(match using_custom_model.remove(0) {
                v if v.ends_with("version=vmaf_v0.6.1") => Self::Vmaf1K,
                v if v.ends_with("version=vmaf_4k_v0.6.1") => Self::Vmaf4K,
                _ => Self::Custom,
            }),
            _ => Some(Self::Custom),
        }
    }
}

#[test]
fn vmaf_lavfi() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(None, PixelFormat::Yuv420p, Some("scale=1280:-1,fps=24")),
        "[0:v]format=yuv420p,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,scale=1280:-1,fps=24,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=n_threads=5:n_subsample=4"
    );
}

#[test]
fn vmaf_lavfi_default() {
    let vmaf = Vmaf {
        vmaf_args: vec![],
        vmaf_scale: VmafScale::Auto,
    };
    let expected = format!(
        "[0:v]format=yuv420p10le,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p10le,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=n_threads={}",
        thread::available_parallelism().map_or(1, |p| p.get())
    );
    assert_eq!(
        vmaf.ffmpeg_lavfi(None, PixelFormat::Yuv420p10le, None),
        expected
    );
}

#[test]
fn vmaf_lavfi_include_n_threads() {
    let vmaf = Vmaf {
        vmaf_args: vec!["log_path=output.xml".into()],
        vmaf_scale: VmafScale::Auto,
    };
    let expected = format!(
        "[0:v]format=yuv420p,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=log_path=output.xml:n_threads={}",
        thread::available_parallelism().map_or(1, |p| p.get())
    );
    assert_eq!(
        vmaf.ffmpeg_lavfi(None, PixelFormat::Yuv420p, None),
        expected
    );
}

/// Low resolution videos should be upscaled to 1080p
#[test]
fn vmaf_lavfi_small_width() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some((1280, 720)), PixelFormat::Yuv420p, None),
        "[0:v]format=yuv420p,scale=1920:-1:flags=bicubic,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,scale=1920:-1:flags=bicubic,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=n_threads=5:n_subsample=4"
    );
}

/// 4k videos should use 4k model
#[test]
fn vmaf_lavfi_4k() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some((3840, 2160)), PixelFormat::Yuv420p, None),
        "[0:v]format=yuv420p,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=n_threads=5:n_subsample=4:model=version=vmaf_4k_v0.6.1"
    );
}

/// >2k videos should be upscaled to 4k & use 4k model
#[test]
fn vmaf_lavfi_3k_upscale_to_4k() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into()],
        vmaf_scale: VmafScale::Auto,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some((3008, 1692)), PixelFormat::Yuv420p, None),
        "[0:v]format=yuv420p,scale=3840:-1:flags=bicubic,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,scale=3840:-1:flags=bicubic,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=n_threads=5:model=version=vmaf_4k_v0.6.1"
    );
}

/// If user has overridden the model, don't default a vmaf width
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
        vmaf.ffmpeg_lavfi(Some((1280, 720)), PixelFormat::Yuv420p, None),
        "[0:v]format=yuv420p,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=model=version=foo:n_threads=5:n_subsample=4"
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
        vmaf.ffmpeg_lavfi(Some((1280, 720)), PixelFormat::Yuv420p, None),
        "[0:v]format=yuv420p,scale=123:-1:flags=bicubic,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,scale=123:-1:flags=bicubic,setpts=PTS-STARTPTS[ref];\
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
        vmaf.ffmpeg_lavfi(Some((1920, 1080)), PixelFormat::Yuv420p, None),
        "[0:v]format=yuv420p,setpts=PTS-STARTPTS[dis];\
         [1:v]format=yuv420p,setpts=PTS-STARTPTS[ref];\
         [dis][ref]libvmaf=n_threads=5:n_subsample=4"
    );
}
