use crate::process::{
    Chunks, FfmpegOut, cmd_err, exit_ok_stderr,
    managed::{ManagedEvent, ManagedProcess},
};
use tokio_stream::{Stream, StreamExt};

#[derive(Debug, PartialEq)]
pub enum ScoreStreamParse {
    Progress(FfmpegOut),
    LogicalDone(Score),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score(f32);

impl Score {
    pub fn new(score: f32) -> Self {
        Self(score)
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LogicalScoreCompletion {
    Pending,
    Done(Score),
}

impl LogicalScoreCompletion {
    fn record(&mut self, event: &ScoreStreamParse) {
        if let ScoreStreamParse::LogicalDone(score) = event {
            *self = Self::Done(*score);
        }
    }

    fn is_done(self) -> bool {
        matches!(self, Self::Done(_))
    }
}

pub fn run_score_stream<Out>(
    process: ManagedProcess,
    name: &'static str,
    cmd_str: String,
    parse_chunk: fn(&[u8], &mut Chunks) -> Option<ScoreStreamParse>,
    into_out: fn(ScoreStreamParse) -> Out,
    into_err: fn(anyhow::Error) -> Out,
) -> impl Stream<Item = Out> {
    let events = process.stderr_events_terminate_on_drop();

    async_stream::stream! {
        let mut chunks = Chunks::default();
        let mut logical_score = LogicalScoreCompletion::Pending;
        tokio::pin!(events);
        while let Some(next) = events.next().await {
            match next {
                Ok(ManagedEvent::RawStderr(chunk)) => {
                    if let Some(event) = parse_chunk(chunk.as_bytes(), &mut chunks) {
                        logical_score.record(&event);
                        yield into_out(event);
                    }
                }
                Ok(ManagedEvent::ReplayGap(_)) => {}
                Ok(ManagedEvent::ProcessDone(done)) => {
                    let status = done.status();
                    if let Err(err) = exit_ok_stderr(name, Ok(status), &cmd_str, &chunks) {
                        yield into_err(err);
                    }
                }
                Err(err) => yield into_err(err),
            }
        }
        if !logical_score.is_done() {
            yield into_err(cmd_err(
                format!("could not parse {name} score"),
                &cmd_str,
                &chunks,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_score_completion_tracks_score_as_a_state() {
        let mut completion = LogicalScoreCompletion::Pending;
        assert!(!completion.is_done());

        completion.record(&ScoreStreamParse::Progress(FfmpegOut::StreamSizes {
            video: 1,
            audio: 0,
            subtitle: 0,
            other: 0,
        }));
        assert!(!completion.is_done());

        completion.record(&ScoreStreamParse::LogicalDone(Score::new(97.5)));
        assert_eq!(completion, LogicalScoreCompletion::Done(Score::new(97.5)));
        assert!(completion.is_done());
    }
}
