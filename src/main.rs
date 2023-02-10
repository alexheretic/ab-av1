mod command;
mod console_ext;
mod ffmpeg;
mod ffprobe;
mod float;
mod process;
mod sample;
mod temporary;
mod vmaf;

use anyhow::anyhow;
use clap::Parser;
use futures::FutureExt;
use std::time::Duration;
use tokio::signal;

const SAMPLE_SIZE_S: u64 = 20;
const SAMPLE_SIZE: Duration = Duration::from_secs(SAMPLE_SIZE_S);

#[derive(Parser)]
#[command(version, about)]
enum Command {
    SampleEncode(command::sample_encode::Args),
    Vmaf(command::vmaf::Args),
    Encode(command::encode::Args),
    CrfSearch(command::crf_search::Args),
    AutoEncode(command::auto_encode::Args),
    PrintCompletions(command::print_completions::Args),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let action = Command::parse();

    let keep = action.keep_temp_files();

    let local = tokio::task::LocalSet::new();

    let command = local.run_until(match action {
        Command::SampleEncode(args) => command::sample_encode(args).boxed_local(),
        Command::Vmaf(args) => command::vmaf(args).boxed_local(),
        Command::Encode(args) => command::encode(args).boxed_local(),
        Command::CrfSearch(args) => command::crf_search(args).boxed_local(),
        Command::AutoEncode(args) => command::auto_encode(args).boxed_local(),
        Command::PrintCompletions(args) => return command::print_completions(args),
    });

    let out = tokio::select! {
        r = command => r,
        _ = signal::ctrl_c() => Err(anyhow!("ctrl_c")),
    };

    temporary::clean(keep).await;

    out
}

impl Command {
    fn keep_temp_files(&self) -> bool {
        match self {
            Self::SampleEncode(args) => args.keep,
            _ => false,
        }
    }
}
