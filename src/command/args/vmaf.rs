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

        let scale_1080p = !args.iter().any(|a| a.contains("model"))
            && distorted_width.map_or(false, |w| w < 1728);

        if scale_1080p {
            // upscale both streams to 1080p, as this resolution is what the default
            // vmaf model is designed to work with
            lavfi.insert_str(
                0,
                "[0:v]scale=1920:-1:flags=bicubic[dis];[1:v]scale=1920:-1:flags=bicubic[ref];[dis][ref]",
            );
        }

        lavfi
    }
}

#[test]
fn vmaf_lavfi() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
    };
    assert_eq!(vmaf.ffmpeg_lavfi(None), "libvmaf=n_threads=5:n_subsample=4");
}

#[test]
fn vmaf_lavfi_default() {
    let vmaf = Vmaf { vmaf_args: vec![] };
    let expected = format!("libvmaf=n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(None), expected);
}

#[test]
fn vmaf_lavfi_include_n_threads() {
    let vmaf = Vmaf {
        vmaf_args: vec!["log_path=output.xml".into()],
    };
    let expected = format!("libvmaf=log_path=output.xml:n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(None), expected);
}

/// Low resolution videos should be upscaled to 1080p
#[test]
fn vmaf_lavfi_small_width() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some(1280)),
        "[0:v]scale=1920:-1:flags=bicubic[dis];\
         [1:v]scale=1920:-1:flags=bicubic[ref];\
         [dis][ref]libvmaf=n_threads=5:n_subsample=4"
    );
}

/// If user has overriden the model, don't presume to scale
#[test]
fn vmaf_lavfi_small_width_custom_model() {
    let vmaf = Vmaf {
        vmaf_args: vec![
            "model=version=foo".into(),
            "n_threads=5".into(),
            "n_subsample=4".into(),
        ],
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some(1280)),
        "libvmaf=model=version=foo:n_threads=5:n_subsample=4"
    );
}

#[test]
fn vmaf_lavfi_1080p() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
    };
    assert_eq!(
        vmaf.ffmpeg_lavfi(Some(1920)),
        "libvmaf=n_threads=5:n_subsample=4"
    );
}
