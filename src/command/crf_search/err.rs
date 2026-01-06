use crate::command::crf_search::Sample;
use crate::command::sample_encode::ScoreKind;
use crate::float::TerseF32;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    NoGoodCrf {
        last: Sample,
        min_score: f32,
        max_encoded_percent: f32,
        score_kind: ScoreKind,
    },
    Other(anyhow::Error),
}

impl Error {
    pub fn ensure_other(condition: bool, reason: &'static str) -> Result<(), Self> {
        if !condition {
            return Err(Self::Other(anyhow::anyhow!(reason)));
        }
        Ok(())
    }

    pub fn ensure_or_no_good_crf(
        condition: bool,
        last: &Sample,
        min_score: f32,
        max_encoded_percent: f32,
        score_kind: ScoreKind,
    ) -> Result<(), Self> {
        if !condition {
            return Err(Self::NoGoodCrf {
                last: last.clone(),
                min_score,
                max_encoded_percent,
                score_kind,
            });
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
            Self::NoGoodCrf {
                last,
                min_score,
                max_encoded_percent,
                score_kind,
            } => {
                let score_too_low = last.enc.score < *min_score;
                let encode_too_large = last.enc.encode_percent > *max_encoded_percent as f64;
                let reason = match (score_too_low, encode_too_large) {
                    (true, true) => "score too low and encode too large",
                    (true, false) => "score too low",
                    (false, true) => "encode too large",
                    (false, false) => "unknown",
                };
                write!(
                    f,
                    "Failed to find a suitable crf: {} \
                    (target: {} >= {}, size <= {}%, \
                    best: crf {}, {} {:.2}, size {:.0}%)",
                    reason,
                    score_kind,
                    min_score,
                    max_encoded_percent,
                    TerseF32(last.crf),
                    last.enc.score_kind,
                    last.enc.score,
                    last.enc.encode_percent
                )
            }
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {}
