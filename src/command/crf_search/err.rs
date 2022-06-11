use crate::command::crf_search::Sample;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    NoGoodCrf { last: Sample },
    Other(anyhow::Error),
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

macro_rules! ensure_other {
    ($condition:expr, $reason:expr) => {
        if !$condition {
            return Err($crate::command::crf_search::err::Error::Other(
                anyhow::anyhow!($reason),
            ));
        }
    };
}
pub(crate) use ensure_other;

macro_rules! ensure_or_no_good_crf {
    ($condition:expr, $last_sample:expr) => {
        if !$condition {
            return Err($crate::command::crf_search::err::Error::NoGoodCrf { last: $last_sample });
        }
    };
}
pub(crate) use ensure_or_no_good_crf;
