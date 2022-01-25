use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct TemporaryPath {
    path: PathBuf,
    keep: bool,
}

impl TemporaryPath {
    pub fn keep(mut self, keep: bool) -> Self {
        self.keep = keep;
        self
    }
}

impl From<PathBuf> for TemporaryPath {
    fn from(path: PathBuf) -> Self {
        Self { path, keep: false }
    }
}

impl std::ops::Deref for TemporaryPath {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for TemporaryPath {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}

impl Drop for TemporaryPath {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
