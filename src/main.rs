mod command;
mod console_ext;
mod ffprobe;
mod process;
mod sample;
mod svtav1;
mod temporary;
mod vmaf;
mod yuv;

use anyhow::anyhow;
use clap::Parser;
use futures::FutureExt;
use std::{io, time::Duration};
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
    AutoEncode(command::auto_encode::Args),
    PrintCompletions(command::print_completions::Args),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let Args { action } = Args::parse();

    action.ensure_temp_dir_exists().await?;
    let keep = action.keep_temp_files();

    let local = tokio::task::LocalSet::new();

    let command = local.run_until(match action {
        Action::SampleEncode(args) => command::sample_encode(args).boxed_local(),
        Action::Vmaf(args) => command::vmaf(args).boxed_local(),
        Action::Encode(args) => command::encode(args).boxed_local(),
        Action::CrfSearch(args) => command::crf_search(args).boxed_local(),
        Action::AutoEncode(args) => command::auto_encode(args).boxed_local(),
        Action::PrintCompletions(args) => return command::print_completions(args),
    });

    let out = tokio::select! {
        r = command => r,
        _ = signal::ctrl_c() => Err(anyhow!("ctrl_c")),
    };

    temporary::clean(keep).await;

    out
}

impl Action {
    fn keep_temp_files(&self) -> bool {
        match self {
            Self::SampleEncode(args) => args.keep,
            _ => false,
        }
    }

    async fn ensure_temp_dir_exists(&self) -> io::Result<()> {
        let temp_dir = match self {
            Self::SampleEncode(args) => &args.sample.temp_dir,
            Self::CrfSearch(args) => &args.sample.temp_dir,
            Self::AutoEncode(args) => &args.search.sample.temp_dir,
            _ => &None,
        };
        if let Some(dir) = temp_dir {
            tokio::fs::create_dir_all(dir).await?;
        }
        Ok(())
    }
}
