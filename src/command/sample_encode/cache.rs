//! _sample-encode_ file system caching logic.
use crate::command::args::EncoderArgs;
use anyhow::Context;
use std::{
    ffi::OsStr,
    hash::Hash,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::fs;

/// Return a previous stored encode result for the same sample & args.
pub async fn cached_encode(
    cache: bool,
    sample: &Path,
    input_duration: Duration,
    input_extension: Option<&OsStr>,
    input_size: u64,
    enc_args: &EncoderArgs<'_>,
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
        ),
        enc_args,
    );

    let key = match Key::try_from_hash(hash) {
        Ok(k) => k,
        _ => return (None, None),
    };

    match fs::read(key.path())
        .await
        .ok()
        .and_then(|d| serde_json::from_slice::<super::EncodeResult>(&d).ok())
    {
        Some(mut result) => {
            result.from_cache = true;
            (Some(result), Some(key))
        }
        _ => (None, Some(key)),
    }
}

pub async fn cache_result(key: Key, result: &super::EncodeResult) -> anyhow::Result<()> {
    let data = serde_json::to_vec(result)?;
    let path = key.path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).await?;
    }
    fs::write(path, data).await?;
    Ok(())
}

#[derive(Debug)]
pub struct Key(PathBuf);

impl Key {
    fn try_from_hash(hash: blake3::Hash) -> anyhow::Result<Self> {
        let mut path = dirs::cache_dir().context("no cache dir found")?;
        path.push("ab-av1");
        path.push(hash.to_hex().as_str());
        path.set_extension("json");
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn hash_encode(input_info: impl Hash, enc_args: &EncoderArgs<'_>) -> blake3::Hash {
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
