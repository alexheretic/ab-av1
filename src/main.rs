mod ffmpeg;
mod ffprobe;
mod svtav1;
mod temporary;

use crate::ffprobe::Ffprobe;
use clap::Parser;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use termcolor::{ColorChoice, ColorSpec, StandardStream, WriteColor};
use tokio::fs;

const SAMPLE_SIZE_S: u64 = 20;
const SAMPLE_SIZE: Duration = Duration::from_secs(SAMPLE_SIZE_S);

#[derive(Parser, Debug)]
#[clap(version, about)]
struct Args {
    /// Input video file.
    #[clap(short)]
    input: PathBuf,

    /// Encoder constant rate factor. Lower means better quality.
    #[clap(long)]
    crf: u8,

    /// Encoder preset. Higher presets means faster encodes, but with a quality tradeoff.
    #[clap(long)]
    preset: u8,

    /// Number of 20s samples.
    #[clap(long, default_value_t = 3)]
    samples: u64,

    /// Don't print verbose progress info.
    #[clap(short, long)]
    quiet: bool,

    /// Keep temporary files after exiting.
    #[clap(long)]
    keep: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let main = Instant::now();
    let Args {
        input,
        crf,
        preset,
        samples,
        quiet,
        keep,
    } = Args::parse();

    macro_rules! verboseprintln {
        ($($x:tt)*) => {
            if !quiet {
                eprintln!($($x)*);
            }
        }
    }

    let Ffprobe { duration } = ffprobe::probe(&input)?;
    let samples = samples.min(duration.as_secs() / SAMPLE_SIZE_S);

    let mut stderr = StandardStream::stderr(ColorChoice::Auto);
    stderr.set_color(ColorSpec::new().set_dimmed(true))?;

    let mut results = Vec::new();
    for sample in 1..=samples {
        let sample_start =
            Duration::from_secs((duration.as_secs() - SAMPLE_SIZE_S * samples) / (samples + 1))
                * sample as _
                + SAMPLE_SIZE * (sample - 1) as _;
        verboseprintln!(
            "==> Sampling {sample_start:?}..{:?} ({sample}/{samples})",
            sample_start + Duration::from_secs(SAMPLE_SIZE_S),
        );

        // cut sample
        let b = Instant::now();
        let sample = ffmpeg::cut_sample(&input, sample_start).await?.keep(keep);
        let sample_size = fs::metadata(&sample).await?.len();
        verboseprintln!(
            " - cut sample {} in {:.1?}",
            sample
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default(),
            b.elapsed()
        );

        // encode sample
        let b = Instant::now();
        let encoded_sample = svtav1::encode(&sample, crf, preset).await?.keep(keep);
        let encode_time = b.elapsed();
        let encoded_size = fs::metadata(&encoded_sample).await?.len();
        verboseprintln!(
            " - encoded {} :{}% in {:.1?}",
            encoded_sample
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default(),
            (encoded_size as f32 * 100.0 / sample_size as f32).round(),
            encode_time
        );

        // calculate vmaf
        let b = Instant::now();
        let vmaf_score = ffmpeg::vmaf(&sample, &encoded_sample).await?;
        verboseprintln!(" - vmaf {vmaf_score} calculated in {:.1?}", b.elapsed());
        results.push(EncodeResult {
            vmaf_score,
            sample_size,
            encoded_size,
            encode_time,
        });
    }
    verboseprintln!();

    // print overall stats
    let input_size = fs::metadata(&input).await?.len();
    eprintln!("Finished in {:.1?}", main.elapsed());
    eprintln!("Encoded size {}%", results.encoded_percent_size().round(),);
    eprintln!(
        "Predicted full encode size {} in {}\n",
        human_bytes::human_bytes(results.encoded_percent_size() * input_size as f64 / 100.0),
        humantime::format_duration(results.estimate_encode_time(duration)),
    );

    verboseprintln!("\nffmpeg -loglevel panic -i {:?} -strict -1 -f yuv4mpegpipe - |\n  \
        SvtAv1EncApp -i stdin -b stdout --crf {} --progress 0 --preset {} |\n  \
        ffpb -i - -i {:?} -map 0:v -map 1:a:0 -c:v copy -c:a libopus -movflags +faststart out.mp4\n",
        input, crf, preset, input);

    stderr.reset()?;
    // print the mean sample vmaf
    println!("{}", results.mean_vmaf());
    Ok(())
}

struct EncodeResult {
    sample_size: u64,
    encoded_size: u64,
    vmaf_score: f32,
    encode_time: Duration,
}

trait EncodeResults {
    fn encoded_percent_size(&self) -> f64;
    fn mean_vmaf(&self) -> f32;
    fn estimate_encode_time(&self, input_duration: Duration) -> Duration;
}
impl EncodeResults for Vec<EncodeResult> {
    fn encoded_percent_size(&self) -> f64 {
        let encoded = self.iter().map(|r| r.encoded_size).sum::<u64>() as f64;
        let sample = self.iter().map(|r| r.sample_size).sum::<u64>() as f64;
        encoded * 100.0 / sample
    }

    fn mean_vmaf(&self) -> f32 {
        self.iter().map(|r| r.vmaf_score).sum::<f32>() / self.len() as f32
    }

    fn estimate_encode_time(&self, input_duration: Duration) -> Duration {
        let sample_factor =
            input_duration.as_secs_f64() / (SAMPLE_SIZE_S as f64 * self.len() as f64);

        let sample_encode_time: f64 = self.iter().map(|r| r.encode_time.as_secs_f64()).sum();

        let estimate = Duration::from_secs_f64(sample_encode_time * sample_factor);
        if estimate < Duration::from_secs(1) {
            estimate
        } else {
            Duration::from_secs(estimate.as_secs())
        }
    }
}
