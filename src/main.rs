mod command;
mod console_ext;
mod ffmpeg;
mod ffprobe;
mod float;
mod log;
mod process;
mod sample;
mod temporary;
mod vmaf;
mod xpsnr;

use ::log::LevelFilter;
use anyhow::anyhow;
use clap::Parser;
use futures_util::FutureExt;
use std::io::IsTerminal;
use tokio::signal;

#[derive(Parser)]
#[command(version, about)]
enum Command {
    SampleEncode(command::sample_encode::Args),
    Vmaf(command::vmaf::Args),
    Xpsnr(command::xpsnr::Args),
    Encode(command::encode::Args),
    CrfSearch(command::crf_search::Args),
    AutoEncode(command::auto_encode::Args),
    PrintCompletions(command::print_completions::Args),
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::builder()
        .filter_module(
            "ab_av1",
            match std::io::stderr().is_terminal() {
                true => LevelFilter::Off,
                false => LevelFilter::Info,
            },
        )
        .parse_default_env()
        .init();

    let action = Command::parse();
    let keep = action.keep_temp_files();

    let local = tokio::task::LocalSet::new();
    let command = local.run_until(match action {
        Command::SampleEncode(args) => command::sample_encode(args).boxed_local(),
        Command::Vmaf(args) => command::vmaf(args).boxed_local(),
        Command::Xpsnr(args) => command::xpsnr(args).boxed_local(),
        Command::Encode(args) => command::encode(args).boxed_local(),
        Command::CrfSearch(args) => command::crf_search(args).boxed_local(),
        Command::AutoEncode(args) => command::auto_encode(args).boxed_local(),
        Command::PrintCompletions(args) => return command::print_completions(args),
    });

    let out = tokio::select! {
        r = command => r,
        _ = signal::ctrl_c() => Err(anyhow!("ctrl_c")),
    };

    // Final cleanup. Samples are already deleted (if wished by the user) during `command::sample_encode::run`.
    temporary::clean(keep).await;

    if let Err(err) = out {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

impl Command {
    /// This decides what commands will keep temp files.
    ///
    /// # Important
    ///
    /// Add commands using the sample sub-args here referencing the `keep` flag,
    /// or the temp files will be removed anyways.
    fn keep_temp_files(&self) -> bool {
        match self {
            Self::SampleEncode(args) => args.sample.keep,
            Self::CrfSearch(args) => args.sample.keep,
            Self::AutoEncode(args) => args.search.sample.keep,
            _ => false,
        }
    }
}
