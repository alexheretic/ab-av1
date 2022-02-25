use clap::Parser;
use std::sync::Arc;

/// Common vmaf options.
#[derive(Parser, Clone)]
pub struct Vmaf {
    /// Additional vmaf arg(s). E.g. --vmaf n_threads=8 --vmaf n_subsample=4
    ///
    /// See https://ffmpeg.org/ffmpeg-filters.html#libvmaf.
    #[clap(long = "vmaf", parse(from_str = parse_vmaf_arg))]
    pub vmaf_args: Vec<Arc<str>>,

    /// Video resolution width to use in VMAF analysis. If set, video streams will be bicupic
    /// scaled to this width during VMAF analysis. By default automatically set based on the
    /// model and input video resolution. Setting to `0` disables any such scaling.
    ///
    /// Default automatic behaviour:
    /// * Video width <  1728 => scale to 1920 (1080p)
    /// * Video width >= 1728 => no scaling
    ///
    /// Scaling happens after any input/reference vfilters.
    #[clap(long)]
    pub vmaf_width: Option<u32>,
}

fn parse_vmaf_arg(arg: &str) -> Arc<str> {
    arg.to_owned().into()
}

impl Vmaf {
    /// Returns ffmpeg `filter_complex`/`lavfi` value for calculating vmaf.
    pub fn ffmpeg_lavfi(&self, distorted_width: Option<u32>) -> String {
        let mut args = self.vmaf_args.clone();
        if !args.iter().any(|a| a.contains("n_threads")) {
            // default n_threads to all cores
            args.push(format!("n_threads={}", num_cpus::get()).into());
        }
        let mut lavfi = args.join(":");
        lavfi.insert_str(0, "libvmaf=");

        let vmaf_w = match (self.vmaf_width, distorted_width) {
            (None, Some(w)) => match VmafModel::from_args(&args) {
                // upscale small resolutions to 1k for use with the 1k model
                VmafModel::Vmaf1K if w < 1728 => Some(1920),
                _ => None,
            },
            (w, _) => w.filter(|w| *w != 0 && Some(*w) != distorted_width),
        };

        if let Some(w) = vmaf_w {
            // scale both streams to the vmaf width
            lavfi.insert_str(
                0,
                &format!("[0:v]scale={w}:-1:flags=bicubic[dis];[1:v]scale={w}:-1:flags=bicubic[ref];[dis][ref]"),
            );
        }

        lavfi
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
        vmaf_width: None,
    };
    assert_eq!(vmaf.ffmpeg_lavfi(None), "libvmaf=n_threads=5:n_subsample=4");
}

#[test]
fn vmaf_lavfi_default() {
    let vmaf = Vmaf {
        vmaf_args: vec![],
        vmaf_width: None,
    };
    let expected = format!("libvmaf=n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(None), expected);
}

#[test]
fn vmaf_lavfi_include_n_threads() {
    let vmaf = Vmaf {
        vmaf_args: vec!["log_path=output.xml".into()],
        vmaf_width: None,
    };
    let expected = format!("libvmaf=log_path=output.xml:n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(None), expected);
}

/// Low resolution videos should be upscaled to 1080p
#[test]
fn vmaf_lavfi_small_width() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_width: None,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some(1280)),
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
        vmaf_width: None,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some(1280)),
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
        vmaf_width: Some(123),
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some(1280)),
        "[0:v]scale=123:-1:flags=bicubic[dis];\
        [1:v]scale=123:-1:flags=bicubic[ref];\
        [dis][ref]libvmaf=model=version=foo:n_threads=5:n_subsample=4"
    );
}

#[test]
fn vmaf_lavfi_1080p() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
        vmaf_width: None,
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some(1920)),
        "libvmaf=n_threads=5:n_subsample=4"
    );
}
