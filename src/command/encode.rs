use crate::{
    command::{
        PROGRESS_CHARS, SmallDuration,
        args::{self, Encoder},
    },
    console_ext::style,
    ffmpeg,
    ffprobe::{self, Ffprobe},
    log::ProgressLogger,
    process::FfmpegOut,
    temporary::{self, TempKind},
};
use anyhow::{Context, ensure};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use log::info;
use same_file::is_same_file;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::fs;
use tokio_stream::StreamExt;

/// Allowed difference between the input duration & the output duration.
///
/// Encoders may round the final frame duration, so an exact match shouldn't be required.
const DURATION_TOLERANCE: Duration = Duration::from_secs(2);

/// Share of the progress bar taken by `--verify-decode`, as a divisor of the encode
/// length. Decoding is expected to be faster than encoding, so it gets the last 1/3.
const VERIFY_BAR_DIVISOR: u64 = 2;

/// Invoke ffmpeg to encode a video or image.
#[derive(Parser)]
#[group(skip)]
pub struct Args {
    #[clap(flatten)]
    pub args: args::Encode,

    /// Encoder constant rate factor (e.g. 1-63 for svt-av1). Lower means better quality.
    #[arg(long)]
    pub crf: f32,

    #[clap(flatten)]
    pub encode: args::EncodeToOutput,
}

pub async fn encode(args: Args) -> anyhow::Result<()> {
    let bar = ProgressBar::new(1).with_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} {elapsed_precise:.bold} {wide_bar:.cyan/blue} ({msg}eta {eta})")?
            .progress_chars(PROGRESS_CHARS)
    );
    bar.enable_steady_tick(Duration::from_millis(100));

    let probe = ffprobe::probe(&args.args.input);
    run(args, probe.into(), &bar).await
}

pub async fn run(
    Args {
        args,
        crf,
        encode:
            args::EncodeToOutput {
                output,
                audio_codec,
                downmix_to_stereo,
                video_only,
                overwrite_input,
                verify,
                verify_decode,
                verify_duration,
                fail_fast,
            },
    }: Args,
    probe: Arc<Ffprobe>,
    bar: &ProgressBar,
) -> anyhow::Result<()> {
    let defaulting_output = output.is_none();
    let output =
        output.unwrap_or_else(|| default_output_name(&args.input, &args.encoder, probe.is_image));

    anyhow::ensure!(
        overwrite_input || !is_same_file(&output, &args.input).unwrap_or(false),
        "Input and Output are specified as the same file. Not proceeding. \
         Pass in `--overwrite-input` to allow this."
    );

    if defaulting_output {
        let out = shell_escape::escape(output.display().to_string().into());
        bar.println(style!("Encoding {out}").dim().to_string());
    }
    bar.set_message("encoding, ");

    let mut enc_args = args.to_ffmpeg_args(
        crf,
        &probe,
        output
            .extension()
            .and_then(|e| e.to_str())
            .context("no output extension?")?,
    )?;
    enc_args.video_only = video_only;
    let has_audio = probe.has_audio;
    let verify_decode = verify || verify_decode;
    let verify_duration = verify || verify_duration;
    if let Ok(d) = &probe.duration {
        let mut len = d.as_micros_u64();
        if verify_decode {
            // verify decode is a last part of the bar, expected to be faster than encoding
            len += len / VERIFY_BAR_DIVISOR;
        }
        bar.set_length(len.max(1));
    }

    // only downmix if achannels > 3
    let stereo_downmix = downmix_to_stereo && probe.max_audio_channels.is_some_and(|c| c > 3);
    let audio_codec = audio_codec.as_deref();
    if stereo_downmix && audio_codec == Some("copy") {
        anyhow::bail!("--stereo-downmix cannot be used with --acodec copy");
    }

    info!(
        "encoding {}",
        output.file_name().and_then(|n| n.to_str()).unwrap_or("")
    );

    let tmp_output = tmp_output_name(&output)?;
    temporary::add(&tmp_output, TempKind::NotKeepable);

    let mut enc = ffmpeg::encode(
        enc_args,
        &tmp_output,
        has_audio,
        audio_codec,
        stereo_downmix,
        fail_fast,
    )?;
    let mut logger = ProgressLogger::new(module_path!(), Instant::now());
    let mut stream_sizes = None;
    while let Some(progress) = enc.next().await {
        match progress? {
            FfmpegOut::Progress { fps, time, .. } => {
                if fps > 0.0 {
                    bar.set_message(format!("{fps} fps, "));
                }
                if let Ok(d) = &probe.duration {
                    bar.set_position(time.as_micros_u64());
                    logger.update(*d, time, fps);
                }
            }
            FfmpegOut::StreamSizes {
                video,
                audio,
                subtitle,
                other,
            } => stream_sizes = Some((video, audio, subtitle, other)),
        }
    }
    enc.wait().await?; // ensure process has exited

    // verified before moving into place, so a failed check leaves no output behind
    if verify_decode {
        bar.set_message("verifying, ");
        let encode_len = probe.duration.as_ref().ok().map(|d| d.as_micros_u64());
        ffmpeg::decode(&tmp_output, |progress| {
            if let FfmpegOut::Progress { fps, time, .. } = progress {
                match fps > 0.0 {
                    true => bar.set_message(format!("verifying {fps} fps, ")),
                    false => bar.set_message("verifying, "),
                }
                if let Some(len) = encode_len {
                    bar.set_position(len + time.as_micros_u64() / VERIFY_BAR_DIVISOR);
                }
            }
        })
        .await?;
    }
    if verify_duration
        && let Ok(expected) = &probe.duration
        // zero means no declared duration, e.g. images & raw streams
        && !expected.is_zero()
    {
        let actual = ffprobe::probe(&tmp_output).duration?;
        ensure!(
            expected.abs_diff(actual) <= DURATION_TOLERANCE,
            "verify: output duration {} does not match input duration {}",
            humantime::format_duration(floor_ms(actual)),
            humantime::format_duration(floor_ms(*expected)),
        );
    }
    bar.finish();

    std::fs::rename(&tmp_output, &output)?;
    temporary::unadd(&tmp_output);

    // print output info
    let output_size = fs::metadata(&output).await?.len();
    let output_percent = 100.0 * output_size as f64 / fs::metadata(&args.input).await?.len() as f64;
    let output_size = style(HumanBytes(output_size)).dim().bold();
    let output_percent = style!("{}%", output_percent.round()).dim().bold();
    eprint!(
        "{} {output_size} {}{output_percent}",
        style("Encoded").dim(),
        style("(").dim(),
    );
    if let Some((video, audio, subtitle, other)) = stream_sizes
        && (audio > 0 || subtitle > 0 || other > 0)
    {
        for (label, size) in [
            ("video:", video),
            ("audio:", audio),
            ("subs:", subtitle),
            ("other:", other),
        ] {
            if size > 0 {
                let size = style(HumanBytes(size)).dim();
                eprint!("{} {}{size}", style(",").dim(), style(label).dim(),);
            }
        }
    }
    eprintln!("{}", style(")").dim());

    Ok(())
}

/// * vid.mp4 -> "mp4"
/// * vid.??? -> "mkv"
/// * image.??? -> "avif"
pub fn default_output_ext(input: &Path, encoder: &Encoder, is_image: bool) -> &'static str {
    if is_image {
        return encoder.default_image_ext();
    }
    match input.extension().and_then(|e| e.to_str()) {
        Some("mp4") => "mp4",
        _ => "mkv",
    }
}

/// E.g. vid.mkv -> "vid.av1.mkv"
pub fn default_output_name(input: &Path, encoder: &Encoder, is_image: bool) -> PathBuf {
    let pre = ffmpeg::pre_extension_name(encoder.as_str());
    let ext = default_output_ext(input, encoder, is_image);
    input.with_extension(format!("{pre}.{ext}"))
}

pub fn tmp_output_name(output: &Path) -> anyhow::Result<PathBuf> {
    let mut tmp_prefix = OsString::from(".tmp.ab-av1-encoding.");
    tmp_prefix.push(output.file_name().context("no output file name")?);
    let mut output = output.to_path_buf();
    output.set_file_name(tmp_prefix);
    Ok(output)
}

/// Drop sub-millisecond parts so durations print readably.
fn floor_ms(duration: Duration) -> Duration {
    Duration::from_millis(duration.as_millis().try_into().unwrap_or(u64::MAX))
}
