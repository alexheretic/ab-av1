use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::{arg, Parser};

use crate::ffmpeg::FfmpegEncodeArgs;
#[derive(Parser)]
#[clap(verbatim_doc_comment)]
#[group(skip, multiple = false)]
pub struct Args {
    /// Reference video file.
    #[arg(long)]
    pub import: Option<PathBuf>,

    /// Ffmpeg video filter applied to the reference before analysis.
    /// E.g. --vfilter "scale=1280:-1,fps=24".
    #[arg(long)]
    pub export: Option<PathBuf>,
}

pub struct EncArgs {
    ffmpeg: FfmpegEncodeArgs,
}

pub async fn cache(Args { import, export }: Args) -> anyhow::Result<()> {
    match (import, export) {
        (_, Some(export)) => {
            let db = open_db();
            Ok(())
        }
        (Some(import), _) => todo!(),
        _ => unreachable!(),
    }
}

pub fn open_db() -> sled::Result<sled::Db> {
    const LOCK_MAX_WAIT: Duration = Duration::from_secs(2);

    let mut path = dirs::cache_dir().expect("no cache dir found");
    path.push("ab-av1");
    path.push("sample-encode-cache");
    let a = Instant::now();
    let mut db = sled::open(&path);
    while db.is_err() && a.elapsed() < LOCK_MAX_WAIT {
        std::thread::yield_now();
        db = sled::open(&path);
    }
    db
}
