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

/// All previously cached sample-encode results for this sample & args, at any crf.
async fn cached_crfs(
    sample: &Path,
    input_duration: Duration,
    input_extension: Option<&OsStr>,
    input_size: u64,
    full_pass: bool,
    enc_args: &FfmpegEncodeArgs<'_>,
    scoring: impl Hash,
) -> Vec<(f32, super::EncodeResult)> {
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
        for entry in db.scan_prefix(prefix) {
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

/// All cached **full** sample-encode outputs for this input & args, at any crf.
///
/// Only crfs are returned where every sample of the run has a cached result, so the
/// aggregated scores are compatible with a real `sample_encode::Output`.
#[allow(clippy::too_many_arguments)]
pub async fn cached_full_outputs(
    input: &Path,
    input_duration: Duration,
    input_extension: Option<&OsStr>,
    input_size: u64,
    input_fps: f64,
    samples: u64,
    sample_duration: Duration,
    enc_args: &FfmpegEncodeArgs<'_>,
    scoring: &impl Hash,
) -> Vec<(f32, super::Output)> {
    use super::{EncodeResults, estimate_encode_size_by_file_percent};
    use std::collections::HashMap;

    let floor_to_sec = sample_duration >= Duration::from_secs(2);
    let frames = ((sample_duration.as_secs_f64() * input_fps).round() as u32).max(1);

    let mut per_sample: Vec<HashMap<u32, super::EncodeResult>> = Vec::new();
    for idx in 0..samples {
        let start = super::sample_start(idx, samples, sample_duration, input_duration);
        let name = crate::sample::sample_file_name(input, start, floor_to_sec, frames);
        let found = cached_crfs(
            Path::new(&name),
            input_duration,
            input_extension,
            input_size,
            false,
            enc_args,
            scoring,
        )
        .await;
        if found.is_empty() {
            return Vec::new();
        }
        per_sample.push(
            found
                .into_iter()
                .map(|(crf, r)| (crf.to_bits(), r))
                .collect(),
        );
    }

    let mut outputs = Vec::new();
    for crf_bits in per_sample[0].keys() {
        if !per_sample[1..].iter().all(|m| m.contains_key(crf_bits)) {
            continue;
        }
        let results: Vec<super::EncodeResult> =
            per_sample.iter().map(|m| m[crf_bits].clone()).collect();
        let predicted_encode_size = results
            .estimate_encode_size_by_duration(input_duration, false)
            .min(
                estimate_encode_size_by_file_percent(&results, input, false)
                    .await
                    .unwrap_or(u64::MAX),
            );
        outputs.push((
            f32::from_bits(*crf_bits),
            super::Output {
                vmaf_score: results.mean_vmaf_score(),
                xpsnr_score: results.mean_xpsnr_score(),
                predicted_encode_size,
                encode_percent: results.encoded_percent_size(),
                predicted_encode_time: results.estimate_encode_time(input_duration, false),
                from_cache: true,
            },
        ));
    }
    outputs.sort_by(|a, b| a.0.total_cmp(&b.0));
    outputs
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

    #[test]
    fn key_prefix_differs_when_input_or_args_differ() {
        let sample = Path::new("movie.sample15+120f.mkv");
        let base = encode_key(
            sample,
            Duration::from_secs(100),
            None,
            1000,
            false,
            &svt_args(30.0),
            (),
        );
        let mut diff_preset = svt_args(30.0);
        diff_preset.preset = Some(Arc::from("6"));
        assert_ne!(
            base.prefix,
            encode_key(
                sample,
                Duration::from_secs(100),
                None,
                1000,
                false,
                &diff_preset,
                ()
            )
            .prefix
        );

        let mut diff_args = svt_args(30.0);
        diff_args
            .output_args
            .push(Arc::new("-svtav1-params".to_string()));
        diff_args.output_args.push(Arc::new("tune=1".to_string()));
        assert_ne!(
            base.prefix,
            encode_key(
                sample,
                Duration::from_secs(100),
                None,
                1000,
                false,
                &diff_args,
                ()
            )
            .prefix
        );

        assert_ne!(
            base.prefix,
            encode_key(
                Path::new("movie.sample60+120f.mkv"),
                Duration::from_secs(100),
                None,
                1000,
                false,
                &svt_args(30.0),
                ()
            )
            .prefix
        );

        assert_ne!(
            base.prefix,
            encode_key(
                sample,
                Duration::from_secs(101),
                None,
                1000,
                false,
                &svt_args(30.0),
                ()
            )
            .prefix
        );

        assert_ne!(
            base.prefix,
            encode_key(
                sample,
                Duration::from_secs(100),
                None,
                1001,
                false,
                &svt_args(30.0),
                ()
            )
            .prefix
        );

        assert_ne!(
            base.prefix,
            encode_key(
                sample,
                Duration::from_secs(100),
                None,
                1000,
                false,
                &svt_args(30.0),
                1u8
            )
            .prefix
        );
    }
}
