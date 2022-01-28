mod command;
mod ffprobe;
mod process;
mod sample;
mod svtav1;
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
#[clap(version, about)]
struct Args {
    #[clap(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    SampleEncode(command::sample_encode::Args),
    Vmaf(command::vmaf::Args),
    Encode(command::encode::Args),
    CrfSearch(command::crf_search::Args),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let Args { action } = Args::parse();
    let mut keep = false;

    let local = tokio::task::LocalSet::new();

    let command = local.run_until(match action {
        Action::SampleEncode(args) => {
            keep = args.keep;
            command::sample_encode(args).boxed_local()
        }
        Action::Vmaf(args) => command::vmaf(args).boxed_local(),
        Action::Encode(args) => command::encode(args).boxed_local(),
        Action::CrfSearch(args) => command::crf_search(args).boxed_local(),
    });

    let out = tokio::select! {
        r = command => r,
        _ = signal::ctrl_c() => Err(anyhow!("ctrl_c")),
    };

    if !keep {
        temporary::clean().await;
    }

    out
}
