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

    /// Keep temporary files after exiting.
    #[clap(long)]
    keep: bool,
}

#[derive(clap::Subcommand)]
enum Action {
    SampleVmaf(command::SampleVmafArgs),
    Vmaf(command::VmafArgs),
    Encode(command::EncodeArgs),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let Args { action, keep } = Args::parse();

    let out = tokio::select! {
        r = match action {
            Action::SampleVmaf(args) => command::sample_vmaf(args).boxed_local(),
            Action::Vmaf(args) => command::vmaf(args).boxed_local(),
            Action::Encode(args) => command::encode(args).boxed_local(),
        } => r,
        _ = signal::ctrl_c() => Err(anyhow!("ctrl_c")),
    };

    if !keep {
        temporary::clean().await;
    }

    out
}
