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
#[allow(clippy::too_many_arguments)]
pub async fn cached_encode(
    cache: bool,
    sample: &Path,
    input_duration: Duration,
    input_extension: Option<&OsStr>,
    input_size: u64,
    full_pass: bool,
    enc_args: &FfmpegEncodeArgs<'_>,
    scoring: impl Hash,
) -> (Option<super::EncodeResult>, Option<Key>) {
    if !cache {
        return (None, None);
    }

    let key = encode_key(
        sample,
        input_duration,
        input_extension,
        input_size,
        full_pass,
        enc_args,
        scoring,
    );

    let cached = tokio::task::spawn_blocking::<_, anyhow::Result<_>>(move || {
        let db = open_db()?;
        Ok(match db.get(key.db_key())? {
            Some(data) => Some(serde_json::from_slice::<super::EncodeResult>(&data)?),
            None => None,
        })
    })
    .await
    .context("db.get task failed")
    .and_then(|r| r);

    match cached {
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

/// All previously cached sample-encode results for this input & args, at any crf.
#[allow(clippy::too_many_arguments)]
pub async fn cached_crfs(
    cache: bool,
    sample: &Path,
    input_duration: Duration,
    input_extension: Option<&OsStr>,
    input_size: u64,
    full_pass: bool,
    enc_args: &FfmpegEncodeArgs<'_>,
    scoring: impl Hash,
) -> Vec<(f32, super::EncodeResult)> {
    if !cache {
        return Vec::new();
    }

    let prefix = *encode_key(
        sample,
        input_duration,
        input_extension,
        input_size,
        full_pass,
        enc_args,
        scoring,
    )
    .prefix
    .as_bytes();

    let scanned = tokio::task::spawn_blocking(move || {
        let db = open_db()?;
        let mut results = Vec::new();
        for entry in db.scan_prefix(&prefix) {
            let (db_key, data) = entry?;
            if let Some(crf) = db_key
                .get(prefix.len()..)
                .and_then(|c| <[u8; 4]>::try_from(c).ok())
                .map(f32::from_be_bytes)
                && let Ok(mut result) = serde_json::from_slice::<super::EncodeResult>(&data)
            {
                result.from_cache = true;
                results.push((crf, result));
            }
        }
        Ok::<_, anyhow::Error>(results)
    })
    .await
    .context("db.scan_prefix task failed")
    .and_then(|r| r);

    match scanned {
        Ok(results) => results,
        Err(err) => {
            eprintln!("cache error: {err}");
            Vec::new()
        }
    }
}

pub async fn cache_result(key: Key, result: &super::EncodeResult) -> anyhow::Result<()> {
    let data = serde_json::to_vec(result)?;
    let insert = tokio::task::spawn_blocking(move || {
        let db = open_db()?;
        db.insert(key.db_key(), data)?;
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

/// Cache db key: a prefix hash over input+sample+args+scoring (crf independent),
/// then the crf as big-endian bits, so all cached crfs of an input are one
/// `scan_prefix` away and ordered by crf (positive f32 bits sort like the float).
#[derive(Debug, Clone, Copy)]
pub struct Key {
    prefix: blake3::Hash,
    crf: f32,
}

impl Key {
    fn db_key(&self) -> [u8; 36] {
        let mut key = [0u8; 36];
        key[..32].copy_from_slice(self.prefix.as_bytes());
        key[32..].copy_from_slice(&self.crf.to_be_bytes());
        key
    }
}

fn encode_key(
    sample: &Path,
    input_duration: Duration,
    input_extension: Option<&OsStr>,
    input_size: u64,
    full_pass: bool,
    enc_args: &FfmpegEncodeArgs<'_>,
    scoring: impl Hash,
) -> Key {
    Key {
        prefix: hash_encode(
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
            scoring,
        ),
        crf: enc_args.crf,
    }
}

fn hash_encode(
    input_info: impl Hash,
    enc_args: &FfmpegEncodeArgs<'_>,
    scoring_info: impl Hash,
) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    let mut std_hasher = BlakeStdHasher(&mut hasher);
    input_info.hash(&mut std_hasher);
    enc_args.sample_encode_hash(&mut std_hasher);
    scoring_info.hash(&mut std_hasher);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn svt_args(crf: f32) -> FfmpegEncodeArgs<'static> {
        FfmpegEncodeArgs {
            input: Path::new("/tmp/movie.mkv"),
            vcodec: Arc::from("libsvtav1"),
            vfilter: None,
            pix_fmt: None,
            crf,
            preset: None,
            output_args: vec![
                Arc::new("-svtav1-params".to_string()),
                Arc::new(format!("scd=1:crf={crf}")),
            ],
            input_args: vec![],
            video_only: true,
        }
    }

    #[test]
    fn key_prefix_ignores_crf_but_db_key_orders_by_it() {
        let sample = Path::new("movie.sample15+120f.mkv");
        let low = encode_key(
            sample,
            Duration::from_secs(100),
            None,
            1000,
            false,
            &svt_args(30.0),
            (),
        );
        let high = encode_key(
            sample,
            Duration::from_secs(100),
            None,
            1000,
            false,
            &svt_args(42.5),
            (),
        );
        assert_eq!(low.prefix, high.prefix);
        assert!(low.db_key() < high.db_key());
    }
}
