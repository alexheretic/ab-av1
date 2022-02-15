//! temp file logic
use once_cell::sync::Lazy;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

static TEMPS: Lazy<Mutex<HashMap<PathBuf, TempKind>>> = Lazy::new(<_>::default);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempKind {
    /// Should always be deleted at the end of the program.
    NotKeepable,
    /// Usually deleted but may be kept, e.g. with --keep.
    Keepable,
}

/// Add a file as temporary so it can be deleted later.
pub fn add(file: impl Into<PathBuf>, kind: TempKind) {
    TEMPS.lock().unwrap().insert(file.into(), kind);
}

/// Remove a previously added file so that it won't be deleted later,
/// if it hasn't already.
pub fn unadd(file: &Path) -> bool {
    TEMPS.lock().unwrap().remove(file).is_some()
}

/// Delete all added temporary files.
/// If `keep_keepables` true don't delete [`TempKind::Keepable`] temporary files.
pub async fn clean(keep_keepables: bool) {
    match keep_keepables {
        true => clean_non_keepables().await,
        false => clean_all().await,
    }
}

/// Delete all added temporary files.
pub async fn clean_all() {
    for (file, _) in std::mem::take(&mut *TEMPS.lock().unwrap()) {
        let _ = tokio::fs::remove_file(file).await;
    }
}

async fn clean_non_keepables() {
    let matching: Vec<_> = TEMPS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, k)| **k == TempKind::NotKeepable)
        .map(|(f, _)| f.clone())
        .collect();

    for file in matching {
        let _ = tokio::fs::remove_file(&file).await;
        TEMPS.lock().unwrap().remove(&file);
    }
}
