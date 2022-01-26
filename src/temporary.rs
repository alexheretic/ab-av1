//! temp file logic
use once_cell::sync::Lazy;
use std::{path::PathBuf, sync::Mutex};

static TEMPS: Lazy<Mutex<Vec<PathBuf>>> = Lazy::new(<_>::default);

/// Add a file as tempoary so it can be removed later.
pub fn add(file: impl Into<PathBuf>) {
    TEMPS.lock().unwrap().push(file.into())
}

/// Delete all added temporary files.
pub async fn clean() {
    for file in std::mem::take(&mut *TEMPS.lock().unwrap()) {
        let _ = tokio::fs::remove_file(file).await;
    }
}
