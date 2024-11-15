use crate::command::crf_search::Sample;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    NoGoodCrf { last: Sample },
    Other(anyhow::Error),
}

impl Error {
    pub fn ensure_other(condition: bool, reason: &'static str) -> Result<(), Self> {
        if !condition {
            return Err(Self::Other(anyhow::anyhow!(reason)));
        }
        Ok(())
    }

    pub fn ensure_or_no_good_crf(condition: bool, last: &Sample) -> Result<(), Self> {
        if !condition {
            return Err(Self::NoGoodCrf { last: last.clone() });
        }
        Ok(())
    }
}

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Other(err.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGoodCrf { .. } => "Failed to find a suitable crf".fmt(f),
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {}
