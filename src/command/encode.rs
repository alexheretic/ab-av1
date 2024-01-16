use crate::{
    command::{
        args::{self, Encoder, OnDuplicate},
        SmallDuration, PROGRESS_CHARS,
    },
    console_ext::style,
    ffmpeg,
    ffprobe::{self, Ffprobe},
    process::FfmpegOut,
    temporary::{self, TempKind},
};
use anyhow::{anyhow, bail};
use clap::Parser;
use console::style;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::fs;
use tokio_stream::StreamExt;

/// Invoke ffmpeg to encode a video or image.
#[derive(Parser)]
#[group(skip)]
pub struct Args {
    #[clap(flatten)]
    pub args: args::Encode,

    /// Encoder constant rate factor (1-63). Lower means better quality.
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
                on_duplicate,
                audio_codec,
                downmix_to_stereo,
                video_only,
            },
    }: Args,
    probe: Arc<Ffprobe>,
    bar: &ProgressBar,
) -> anyhow::Result<()> {
    // let probe = ffprobe::probe(&args.input);
    let output =
        output.unwrap_or_else(|| default_output_name(&args.input, &args.encoder, probe.is_image));
    let on_duplicate = on_duplicate.unwrap_or(OnDuplicate::default());

    let output = match on_duplicate {
        OnDuplicate::Overwrite => output,
        OnDuplicate::Rename => rename_if_exists(output)?,
        OnDuplicate::Skip => {
            if output.exists() {
                bar.println(format!(
                    "{} {}",
                    style("Skipping").dim(),
                    style(output.display()).dim(),
                ));
                bail!("Output file already exists");
            } else {
                output
            }
        }
        OnDuplicate::Ask => {
            if output.exists() {
                /*bar.
                ));*/
                bar.suspend(|| loop {
                    let mut input = String::new();
                    print!(
                        "{} {}. {}",
                        style("Output file already exists:"),
                        style(output.display()).dim(),
                        style("Overwrite, rename, skip, or quit? [o/r/s/q]").italic()
                    );
                    std::io::stdout().flush()?;
                    std::io::stdin().read_line(&mut input)?;
                    match input.trim() {
                        "o" | "overwrite" => {
                            break Ok(output);
                        }
                        "r" | "rename" => {
                            break rename_if_exists(output);
                        }
                        "s" | "skip" => {
                            bail!("Output file already exists");
                        }
                        "q" | "quit" => {
                            bail!("User quit");
                        }
                        _ => {
                            eprintln!("Invalid input");
                        }
                    }
                })?
            } else {
                output
            }
        }
    };

    // output is temporary until encoding has completed successfully
    temporary::add(&output, TempKind::NotKeepable);

    let out = shell_escape::escape(output.display().to_string().into());
    bar.println(style!("Encoding {out}").dim().to_string());
    bar.set_message("encoding, ");

    let mut enc_args = args.to_encoder_args(crf, &probe)?;
    enc_args.video_only = video_only;
    let has_audio = probe.has_audio;
    if let Ok(d) = &probe.duration {
        bar.set_length(d.as_micros_u64().max(1));
    }

    // only downmix if achannels > 3
    let stereo_downmix = downmix_to_stereo && probe.max_audio_channels.map_or(false, |c| c > 3);
    let audio_codec = audio_codec.as_deref();
    if stereo_downmix && audio_codec == Some("copy") {
        anyhow::bail!("--stereo-downmix cannot be used with --acodec copy");
    }

    let mut enc = ffmpeg::encode(enc_args, &output, has_audio, audio_codec, stereo_downmix)?;

    let mut stream_sizes = None;
    while let Some(progress) = enc.next().await {
        match progress? {
            FfmpegOut::Progress { fps, time, .. } => {
                if fps > 0.0 {
                    bar.set_message(format!("{fps} fps, "));
                }
                if probe.duration.is_ok() {
                    bar.set_position(time.as_micros_u64());
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
    bar.finish();

    // successful encode, so don't delete it!
    temporary::unadd(&output);

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
    if let Some((video, audio, subtitle, other)) = stream_sizes {
        if audio > 0 || subtitle > 0 || other > 0 {
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
    }
    eprintln!("{}", style(")").dim());

    Ok(())
}

fn rename_if_exists(mut output: PathBuf) -> anyhow::Result<PathBuf> {
    while output.exists() {
        // get basename without extension, or full name in case that fails
        let name = output
            .file_stem()
            .or_else(|| output.file_name())
            .ok_or(anyhow!("Could not parse file name from {:?}", output))?
            .to_string_lossy();

        // if the last part of the file stem after an underscore is a valid positive integer,
        // increment it by one. Otherwise add an "_1" suffix.
        let mut parts: Vec<String> = name.split('_').map(&str::to_owned).collect();
        if let Some(last_part) = parts.pop() {
            if let Ok(number) = last_part.parse::<u32>() {
                parts.push((number + 1).to_string());
            } else {
                parts.push(last_part);
                parts.push("1".to_owned());
            }
        } else {
            // this shouldn't happen since the name would have to be equal to "" (the empty string)
            bail!("Output name vector {:?} should't be empty", parts);
        }

        let name = parts.join("_")
            + &output
                .extension()
                .map(|e| ".".to_owned() + &e.to_string_lossy())
                .unwrap_or("".to_owned());
        output = output.with_file_name(name);
    }
    Ok(output)
}

#[test]
fn test_rename_if_exists() -> anyhow::Result<()> {
    use anyhow::ensure;
    use std::ops::Range;
    use std::panic::catch_unwind;

    let temp_dir = mktemp::Temp::new_dir()?;
    let mut temp_file = PathBuf::new();
    temp_file.push(&temp_dir);

    touch(&temp_file, "test.mkv")?;
    _do(&temp_file, "test.mkv", "test_{}.mkv", 1..13)?;

    touch(&temp_dir, "test_1_2_3")?;
    _do(&temp_file, "test_1_2_1", "test_1_2_{}", 1..3)?;
    assert!(catch_unwind(|| { _do(&temp_file, "test_1_2_1", "test_1_2_{}", 3..4) }).is_err());
    _do(&temp_file, "test_1_2_1", "test_1_2_{}", 4..7)?;

    _do(&temp_file, "test_1_2", "test_1_{}", 2..6)?;

    touch(&temp_file, ".hidden_27")?;
    _do(&temp_file, ".hidden", ".hidden", 0..1)?;
    _do(&temp_file, ".hidden", ".hidden_{}", 1..27)?;
    assert!(catch_unwind(|| { _do(&temp_file, ".hidden", ".hidden_{}", 27..28) }).is_err());
    _do(&temp_file, ".hidden", ".hidden_{}", 28..31)?;

    fn touch(f: &Path, name: &str) -> anyhow::Result<()> {
        let mut p = PathBuf::new();
        p.push(f);
        p.push(name);
        std::process::Command::new("touch").arg(p).output()?;
        Ok(())
    }

    fn _do(temp_dir: &Path, name: &str, pattern: &str, range: Range<usize>) -> anyhow::Result<()> {
        ensure!(range.len() > 0, "range must be non-empty");

        let mut temp_file = PathBuf::new();
        temp_file.push(temp_dir);
        temp_file.push(name);

        for i in range {
            let result = rename_if_exists(temp_file.clone())?;
            let actual = result
                .file_name()
                .map(|s| s.to_string_lossy())
                .unwrap_or("".into());
            let expected = pattern.replace("{}", i.to_string().as_str());
            assert_eq!(actual, expected);

            std::process::Command::new("touch").arg(&result).output()?;
        }
        Ok(())
    }
    Ok(())
}

/// * vid.mp4 -> "mp4"
/// * vid.??? -> "mkv"
/// * image.??? -> "avif"
pub fn default_output_ext(input: &Path, is_image: bool) -> &'static str {
    if is_image {
        return "avif";
    }
    match input.extension().and_then(|e| e.to_str()) {
        Some("mp4") => "mp4",
        _ => "mkv",
    }
}

/// E.g. vid.mkv -> "vid.av1.mkv"
pub fn default_output_name(input: &Path, encoder: &Encoder, is_image: bool) -> PathBuf {
    let pre = ffmpeg::pre_extension_name(encoder.as_str());
    let ext = default_output_ext(input, is_image);
    input.with_extension(format!("{pre}.{ext}"))
}
