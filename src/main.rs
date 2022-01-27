mod command;
mod ffmpeg;
mod ffprobe;
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
    SampleEncode(command::SampleEncodeArgs),
    Vmaf(command::VmafArgs),
    Encode(command::EncodeArgs),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let Args { action } = Args::parse();
    let mut keep = false;

    let command = match action {
        Action::SampleEncode(args) => {
            keep = args.keep;
            command::sample_encode(args).boxed_local()
        }
        Action::Vmaf(args) => command::vmaf(args).boxed_local(),
        Action::Encode(args) => command::encode(args).boxed_local(),
    };

    let out = tokio::select! {
        r = command => r,
        _ = signal::ctrl_c() => Err(anyhow!("ctrl_c")),
    };

    if !keep {
        temporary::clean().await;
    }

    out
}
