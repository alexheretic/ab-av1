//! temp file logic
use once_cell::sync::Lazy;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Mutex,
};

static TEMPS: Lazy<Mutex<HashSet<PathBuf>>> = Lazy::new(<_>::default);

/// Add a file as temporary so it can be deleted later.
pub fn add(file: impl Into<PathBuf>) {
    TEMPS.lock().unwrap().insert(file.into());
}

/// Remove a previously added file so that it won't be deleted later,
/// if it hasn't already.
pub fn unadd(file: &Path) -> bool {
    TEMPS.lock().unwrap().remove(file)
}

/// Delete all added temporary files.
pub async fn clean() {
    for file in std::mem::take(&mut *TEMPS.lock().unwrap()) {
        let _ = tokio::fs::remove_file(file).await;
    }
}
