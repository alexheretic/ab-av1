//! temp file logic
use std::{
    collections::HashMap,
    env, iter,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempKind {
    /// Should always be deleted at the end of the program.
    NotKeepable,
    /// Usually deleted but may be kept, e.g. with --keep.
    Keepable,
}

/// Add a file as temporary so it can be deleted later.
pub fn add(file: impl Into<PathBuf>, kind: TempKind) {
    temp_files().lock().unwrap().insert(file.into(), kind);
}

/// Remove a previously added file so that it won't be deleted later,
/// if it hasn't already.
pub fn unadd(file: &Path) -> bool {
    temp_files().lock().unwrap().remove(file).is_some()
}

fn temp_files() -> &'static Mutex<HashMap<PathBuf, TempKind>> {
    static TEMPS: OnceLock<Mutex<HashMap<PathBuf, TempKind>>> = OnceLock::new();
    TEMPS.get_or_init(<_>::default)
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
    let mut files: Vec<_> = std::mem::take(&mut *temp_files().lock().unwrap())
        .into_keys()
        .collect();
    files.sort_by_key(|f| f.is_dir()); // rm dir at the end

    for file in files {
        match file.is_dir() {
            true => _ = tokio::fs::remove_dir(file).await,
            false => _ = tokio::fs::remove_file(file).await,
        }
    }
}

async fn clean_non_keepables() {
    let mut matching: Vec<_> = temp_files()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, k)| **k == TempKind::NotKeepable)
        .map(|(f, _)| f.clone())
        .collect();
    matching.sort_by_key(|f| f.is_dir()); // rm dir at the end

    for file in matching {
        match file.is_dir() {
            true => _ = tokio::fs::remove_dir(&file).await,
            false => _ = tokio::fs::remove_file(&file).await,
        }
        temp_files().lock().unwrap().remove(&file);
    }
}

/// Return a temporary directory that is distinct per process/run.
///
/// Configured --temp-dir is used as a parent or, if not set, the current working dir.
pub fn process_dir(conf_parent: Option<PathBuf>) -> PathBuf {
    static SUBDIR: OnceLock<String> = OnceLock::new();
    let subdir = SUBDIR.get_or_init(|| {
        let mut subdir = String::from(".ab-av1-");
        subdir.extend(iter::repeat_with(fastrand::alphanumeric).take(12));
        subdir
    });

    let mut temp_dir =
        conf_parent.unwrap_or_else(|| env::current_dir().expect("current working directory"));
    temp_dir.push(subdir);

    if !temp_dir.exists() {
        add(&temp_dir, TempKind::Keepable);
        std::fs::create_dir_all(&temp_dir).expect("failed to create temp-dir");
    }

    temp_dir
}
