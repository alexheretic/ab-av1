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
    pub fn ffmpeg_lavfi(&self) -> String {
        let mut args = self.vmaf_args.clone();
        if !args.iter().any(|a| a.contains("n_threads")) {
            // default n_threads to all cores
            args.push(format!("n_threads={}", num_cpus::get()).into());
        }
        let mut combined = args.join(":");
        combined.insert_str(0, "libvmaf=");
        combined
    }
}

#[test]
fn vmaf_lavfi() {
    let vmaf = Vmaf {
        vmaf_args: vec!["n_threads=5".into(), "n_subsample=4".into()],
    };
    assert_eq!(vmaf.ffmpeg_lavfi(), "libvmaf=n_threads=5:n_subsample=4");
}

#[test]
fn vmaf_lavfi_default() {
    let vmaf = Vmaf { vmaf_args: vec![] };
    let expected = format!("libvmaf=n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(), expected);
}

#[test]
fn vmaf_lavfi_include_n_threads() {
    let vmaf = Vmaf {
        vmaf_args: vec!["log_path=output.xml".into()],
    };
    let expected = format!("libvmaf=log_path=output.xml:n_threads={}", num_cpus::get());
    assert_eq!(vmaf.ffmpeg_lavfi(), expected);
}
