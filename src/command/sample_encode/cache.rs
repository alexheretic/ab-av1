//! _sample-encode_ file system caching logic.
use crate::ffmpeg::FfmpegEncodeArgs;
use anyhow::Context;
use std::{
    ffi::OsStr,
    hash::Hash,
    path::Path,
    time::{Duration, Instant},
};

/// Return a previous stored encode result for the same sample & args.
pub async fn cached_encode(
    cache: bool,
    sample: &Path,
    input_duration: Duration,
    input_extension: Option<&OsStr>,
    input_size: u64,
    full_pass: bool,
    enc_args: &FfmpegEncodeArgs<'_>,
) -> (Option<super::EncodeResult>, Option<Key>) {
    if !cache {
        return (None, None);
    }

    let hash = hash_encode(
        // hashing the sample file name (which includes input name, frames & start)
        // + input duration, extension & size should be reasonably unique for an input.
        // and is much faster than hashing the entire file.
        (
            sample.file_name(),
            input_duration,
            input_extension,
            input_size,
            full_pass,
        ),
        enc_args,
    );

    let key = Key(hash);

    match tokio::task::spawn_blocking::<_, anyhow::Result<_>>(move || {
        let db = open_db()?;
        Ok(match db.get(key.0.to_hex().as_bytes())? {
            Some(data) => Some(serde_json::from_slice::<super::EncodeResult>(&data)?),
            None => None,
        })
    })
    .await
    .context("db.get task failed")
    .and_then(|r| r)
    {
        Ok(Some(mut result)) => {
            result.from_cache = true;
            (Some(result), Some(key))
        }
        Ok(None) => (None, Some(key)),
        Err(err) => {
            eprintln!("cache error: {err}");
            (None, None)
        }
    }
}

pub async fn cache_result(key: Key, result: &super::EncodeResult) -> anyhow::Result<()> {
    let data = serde_json::to_vec(result)?;
    let insert = tokio::task::spawn_blocking(move || {
        let db = open_db()?;
        db.insert(key.0.to_hex().as_bytes(), data)?;
        db.flush()
    })
    .await
    .context("db.insert task failed")
    .and_then(|r| Ok(r?));

    if let Err(err) = insert {
        eprintln!("cache error: {err}")
    }
    Ok(())
}

fn open_db() -> sled::Result<sled::Db> {
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

#[derive(Debug, Clone, Copy)]
pub struct Key(blake3::Hash);

fn hash_encode(input_info: impl Hash, enc_args: &FfmpegEncodeArgs<'_>) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    let mut std_hasher = BlakeStdHasher(&mut hasher);
    input_info.hash(&mut std_hasher);
    enc_args.sample_encode_hash(&mut std_hasher);
    hasher.finalize()
}

struct BlakeStdHasher<'a>(&'a mut blake3::Hasher);
impl std::hash::Hasher for BlakeStdHasher<'_> {
    fn finish(&self) -> u64 {
        unimplemented!()
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}
