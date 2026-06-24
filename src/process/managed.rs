#![allow(
    dead_code,
    reason = "managed process wrapper is introduced before callers migrate onto it"
)]

use anyhow::bail;
use std::process::{ExitStatus, Output};
use std::time::Duration;
use tokio::process::Command;
use tokio_process_tools::{Chunk, visitors::inspect::InspectChunks};
use tokio_process_tools::{
    CollectionOverflowBehavior, Consumable, DEFAULT_MAX_BUFFERED_CHUNKS,
    DEFAULT_OUTPUT_EOF_TIMEOUT, DEFAULT_READ_CHUNK_SIZE, GracefulShutdown,
    LossyWithoutBackpressure, Next, Process, RawCollectionOptions, RawOutputOptions, ReplayEnabled,
    StreamEvent, Subscribable, Subscription,
};
use tokio_process_tools::{NumBytesExt, ProcessHandle, SingleSubscriberOutputStream};
use tokio_stream::Stream;

const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const DEFAULT_TERMINATION_GRACE: Duration = Duration::from_millis(25);
const DEFAULT_STDERR_LIMIT: usize = 32_768;

#[derive(Debug, Clone, Copy)]
pub struct ManagedProcessOptions {
    wait_timeout: Duration,
    termination_grace: Duration,
    stderr_limit: usize,
}

impl Default for ManagedProcessOptions {
    fn default() -> Self {
        Self {
            wait_timeout: DEFAULT_WAIT_TIMEOUT,
            termination_grace: DEFAULT_TERMINATION_GRACE,
            stderr_limit: DEFAULT_STDERR_LIMIT,
        }
    }
}

impl ManagedProcessOptions {
    pub fn with_wait_timeout(mut self, wait_timeout: Duration) -> Self {
        self.wait_timeout = wait_timeout;
        self
    }

    pub fn with_stderr_limit(mut self, stderr_limit: usize) -> Self {
        self.stderr_limit = stderr_limit;
        self
    }
}

pub struct ManagedProcess {
    handle: ProcessHandle<
        SingleSubscriberOutputStream<LossyWithoutBackpressure, ReplayEnabled>,
        SingleSubscriberOutputStream<LossyWithoutBackpressure, ReplayEnabled>,
    >,
    options: ManagedProcessOptions,
}

/// Process policy for streams that must run through process completion.
///
/// Use this for encode/progress streams where dropping before `ProcessDone` is
/// a programming error. Dropping the wrapper while the child is live preserves
/// the underlying process drop guard, so misuse is loud in tests.
pub struct MustCompleteProcess(ManagedProcess);

/// Process policy for streams where the caller may stop after a logical result.
///
/// Use this for score streams: VMAF/XPSNR can produce a logical score before
/// ffmpeg exits. Dropping this wrapper or a stream built from it terminates the
/// child instead of detaching it.
pub struct TerminateOnDropProcess(Option<ManagedProcess>);

impl Drop for TerminateOnDropProcess {
    fn drop(&mut self) {
        let Some(process) = self.0.take() else {
            return;
        };

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        handle.spawn(async move {
            let _ = process.terminate_after(Duration::ZERO).await;
        });
    }
}

/// Bounded terminal output collected after a process has exited.
///
/// Stderr keeps the newest bytes up to the configured limit; when older bytes
/// were dropped, `stderr_truncation` is `Truncated`.
#[derive(Debug)]
pub struct ManagedOutput {
    pub status: ExitStatus,
    pub stderr: Vec<u8>,
    pub stderr_truncation: OutputTruncation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTruncation {
    Complete,
    Truncated,
}

impl OutputTruncation {
    fn from_truncated(truncated: bool) -> Self {
        if truncated {
            Self::Truncated
        } else {
            Self::Complete
        }
    }

    pub fn is_truncated(self) -> bool {
        matches!(self, Self::Truncated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawOutputChunk(Vec<u8>);

impl RawOutputChunk {
    fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessDone(ExitStatus);

impl ProcessDone {
    fn new(status: ExitStatus) -> Self {
        Self(status)
    }

    pub fn status(self) -> ExitStatus {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputReplayGap;

/// Typed process stderr stream event.
///
/// The underlying tokio-process-tools stream replays recent stderr bytes for
/// delayed subscribers and may report `ReplayGap` when lossy buffering skipped
/// data under pressure. `ProcessDone` means the child reached a terminal status;
/// score streams may yield their own logical completion before this event.
#[derive(Debug)]
pub enum ManagedEvent {
    RawStderr(RawOutputChunk),
    ReplayGap(OutputReplayGap),
    ProcessDone(ProcessDone),
}

impl ManagedProcess {
    pub fn spawn(name: &'static str, cmd: Command) -> anyhow::Result<Self> {
        Self::spawn_with_options(name, cmd, ManagedProcessOptions::default())
    }

    pub fn spawn_with_options(
        name: &'static str,
        cmd: Command,
        options: ManagedProcessOptions,
    ) -> anyhow::Result<Self> {
        let handle = Process::new(cmd)
            .name(name)
            .stdout_and_stderr(|stream| {
                stream
                    .single_subscriber()
                    .lossy_without_backpressure()
                    .replay_last_bytes(DEFAULT_STDERR_LIMIT.bytes())
                    .read_chunk_size(DEFAULT_READ_CHUNK_SIZE)
                    .max_buffered_chunks(DEFAULT_MAX_BUFFERED_CHUNKS)
            })
            .spawn()?;
        Ok(Self { handle, options })
    }

    fn graceful_shutdown_for(options: ManagedProcessOptions) -> GracefulShutdown {
        GracefulShutdown::builder()
            .unix_sigterm(options.termination_grace)
            .windows_ctrl_break(options.termination_grace)
            .build()
    }

    pub async fn stderr_chunks(self) -> anyhow::Result<(ExitStatus, Vec<u8>)> {
        let output = self.stderr_output().await?;
        Ok((output.status, output.stderr))
    }

    pub async fn output(self) -> anyhow::Result<Output> {
        let output = self.stderr_output().await?;
        Ok(Output {
            status: output.status,
            stdout: Vec::new(),
            stderr: output.stderr,
        })
    }

    pub async fn stderr_output(mut self) -> anyhow::Result<ManagedOutput> {
        let options = self.options;
        let shutdown = Self::graceful_shutdown_for(options);
        let output = self
            .handle
            .wait_for_completion(options.wait_timeout)
            .with_raw_output(
                DEFAULT_OUTPUT_EOF_TIMEOUT,
                RawOutputOptions::symmetric(RawCollectionOptions::Bounded {
                    max_bytes: options.stderr_limit.bytes(),
                    overflow_behavior: CollectionOverflowBehavior::DropOldestData,
                }),
            )
            .or_terminate(shutdown)
            .await?;
        let Some(output) = output.into_completed() else {
            bail!(
                "process exceeded {:?} and was terminated",
                options.wait_timeout
            );
        };

        Ok(ManagedOutput {
            status: output.status,
            stderr: output.stderr.bytes,
            stderr_truncation: OutputTruncation::from_truncated(output.stderr.truncated),
        })
    }

    pub async fn observe_stderr_chunks(
        mut self,
        on_chunk: impl FnMut(Chunk) -> Next + Send + 'static,
    ) -> anyhow::Result<ExitStatus> {
        let options = self.options;
        let shutdown = Self::graceful_shutdown_for(options);
        let consumer = self
            .handle
            .stderr()
            .consume(InspectChunks::builder().f(on_chunk).build())?;
        let status = self
            .handle
            .wait_for_completion(options.wait_timeout)
            .or_terminate(shutdown)
            .await?;
        let Some(status) = status.into_completed() else {
            bail!(
                "process exceeded {:?} and was terminated",
                options.wait_timeout
            );
        };
        consumer.wait().await?;
        Ok(status)
    }

    /// Select the must-complete streaming policy.
    ///
    /// This is the policy used by encode progress streams: consumers should
    /// drive the stream to `ProcessDone` or call the stream's terminal `wait`.
    pub fn must_complete(self) -> MustCompleteProcess {
        MustCompleteProcess(self)
    }

    /// Select the terminate-on-drop streaming policy.
    ///
    /// This is the policy used by score streams that can stop after a logical
    /// score. Dropping the wrapper or stream schedules child termination.
    pub fn terminate_on_drop(self) -> TerminateOnDropProcess {
        TerminateOnDropProcess(Some(self))
    }

    pub async fn observe_stdout_chunks(
        mut self,
        on_chunk: impl FnMut(Chunk) -> Next + Send + 'static,
    ) -> anyhow::Result<ExitStatus> {
        let options = self.options;
        let shutdown = Self::graceful_shutdown_for(options);
        let consumer = self
            .handle
            .stdout()
            .consume(InspectChunks::builder().f(on_chunk).build())?;
        let status = self
            .handle
            .wait_for_completion(options.wait_timeout)
            .or_terminate(shutdown)
            .await?;
        let Some(status) = status.into_completed() else {
            bail!(
                "process exceeded {:?} and was terminated",
                options.wait_timeout
            );
        };
        consumer.wait().await?;
        Ok(status)
    }

    pub fn id(&self) -> Option<u32> {
        self.handle.id()
    }

    pub async fn terminate_after(mut self, timeout: Duration) -> anyhow::Result<ExitStatus> {
        Ok(self
            .handle
            .wait_for_completion(timeout)
            .or_terminate(
                GracefulShutdown::builder()
                    .unix_sigterm(self.options.termination_grace)
                    .windows_ctrl_break(self.options.termination_grace)
                    .build(),
            )
            .await?
            .into_result())
    }
}

fn managed_event_from_stream_event(event: StreamEvent) -> anyhow::Result<Option<ManagedEvent>> {
    Ok(match event {
        StreamEvent::Chunk(chunk) => Some(ManagedEvent::RawStderr(RawOutputChunk::new(
            chunk.as_ref().to_vec(),
        ))),
        StreamEvent::Gap => Some(ManagedEvent::ReplayGap(OutputReplayGap)),
        StreamEvent::Eof => None,
        StreamEvent::ReadError(err) => Err(err)?,
    })
}

async fn wait_for_process_done(process: &mut ManagedProcess) -> anyhow::Result<ProcessDone> {
    let options = process.options;
    let shutdown = ManagedProcess::graceful_shutdown_for(options);
    let status = process
        .handle
        .wait_for_completion(options.wait_timeout)
        .or_terminate(shutdown)
        .await?;
    let status = match status.into_completed() {
        Some(status) => status,
        None => Err(anyhow::anyhow!(
            "process exceeded {:?} and was terminated",
            options.wait_timeout,
        ))?,
    };
    Ok(ProcessDone::new(status))
}

impl MustCompleteProcess {
    /// Stream stderr chunks and then the terminal process status.
    ///
    /// Read errors and timeout termination are yielded as errors. EOF from
    /// stderr is not success by itself; the stream waits for process completion
    /// and only then yields `ProcessDone`.
    pub fn stderr_events(self) -> impl Stream<Item = anyhow::Result<ManagedEvent>> {
        async_stream::try_stream! {
            let mut process = self.0;
            let mut stderr = process.handle.stderr().try_subscribe()?;
            while let Some(event) = stderr.next_event().await {
                match managed_event_from_stream_event(event)? {
                    Some(ManagedEvent::RawStderr(chunk)) => yield ManagedEvent::RawStderr(chunk),
                    Some(ManagedEvent::ReplayGap(gap)) => yield ManagedEvent::ReplayGap(gap),
                    Some(ManagedEvent::ProcessDone(done)) => yield ManagedEvent::ProcessDone(done),
                    None => break,
                }
            }

            yield ManagedEvent::ProcessDone(wait_for_process_done(&mut process).await?);
        }
    }
}

impl TerminateOnDropProcess {
    /// Stream stderr chunks with cancellation-on-drop semantics.
    ///
    /// If the stream is dropped during stderr streaming or final process wait,
    /// the owned child is terminated. If it reaches `ProcessDone`, the child has
    /// already completed and no cancellation is performed.
    pub fn stderr_events(mut self) -> impl Stream<Item = anyhow::Result<ManagedEvent>> {
        async_stream::try_stream! {
            let mut process = TerminateOnDropProcess(self.0.take());
            let Some(inner) = process.0.as_mut() else {
                return;
            };
            let mut stderr = inner.handle.stderr().try_subscribe()?;
            while let Some(event) = stderr.next_event().await {
                match managed_event_from_stream_event(event)? {
                    Some(ManagedEvent::RawStderr(chunk)) => yield ManagedEvent::RawStderr(chunk),
                    Some(ManagedEvent::ReplayGap(gap)) => yield ManagedEvent::ReplayGap(gap),
                    Some(ManagedEvent::ProcessDone(done)) => yield ManagedEvent::ProcessDone(done),
                    None => break,
                }
            }

            drop(stderr);
            let Some(inner) = process.0.as_mut() else {
                return;
            };
            let done = wait_for_process_done(inner).await?;
            drop(process.0.take());
            yield ManagedEvent::ProcessDone(done);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        io::{self, Write},
        sync::{Arc, Mutex},
        thread,
    };
    use tokio::process::Command;
    use tokio_stream::StreamExt;

    const FIXTURE_ENV: &str = "AB_AV1_MANAGED_PROCESS_FIXTURE";
    const FIXTURE_TEST: &str = "process::managed::tests::managed_process_fixture_child";

    fn fixture_command(fixture: &str) -> Command {
        let mut cmd = Command::new(env::current_exe().expect("current test executable"));
        cmd.arg("--exact")
            .arg(FIXTURE_TEST)
            .arg("--nocapture")
            .env(FIXTURE_ENV, fixture);
        cmd
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    /// Portable process fixtures executed by re-running the current test binary.
    ///
    /// Keep expected output ordering in `expected_sequence` when adding a case;
    /// the catalog test below guards the behaviors needed by streaming tests
    /// without depending on shell, media files, or platform-specific signals.
    enum ManagedProcessFixture {
        StderrProgress,
        StderrWarning,
        StderrDigits,
        StderrOneTwo,
        StderrOneSleepTwo,
        StderrFfmpegProgress,
        StderrFfmpegProgressTwice,
        StderrBadnessExit7,
        StderrManyLinesExit7,
        StdoutNoiseStderrFfmpegProgress,
        StdoutOneSleepTwo,
        SleepLong,
        VmafScoreThenSleep,
        VmafProgressScore,
        VmafNoScore,
        VmafScoreExit7,
        StdoutNoiseVmafProgressScore,
        XpsnrScoreThenSleep,
        XpsnrProgressScore,
        XpsnrNoScore,
        XpsnrScoreExit7,
        StdoutNoiseXpsnrProgressScore,
    }

    impl ManagedProcessFixture {
        const ALL: &'static [Self] = &[
            Self::StderrProgress,
            Self::StderrWarning,
            Self::StderrDigits,
            Self::StderrOneTwo,
            Self::StderrOneSleepTwo,
            Self::StderrFfmpegProgress,
            Self::StderrFfmpegProgressTwice,
            Self::StderrBadnessExit7,
            Self::StderrManyLinesExit7,
            Self::StdoutNoiseStderrFfmpegProgress,
            Self::StdoutOneSleepTwo,
            Self::SleepLong,
            Self::VmafScoreThenSleep,
            Self::VmafProgressScore,
            Self::VmafNoScore,
            Self::VmafScoreExit7,
            Self::StdoutNoiseVmafProgressScore,
            Self::XpsnrScoreThenSleep,
            Self::XpsnrProgressScore,
            Self::XpsnrNoScore,
            Self::XpsnrScoreExit7,
            Self::StdoutNoiseXpsnrProgressScore,
        ];

        fn from_name(name: &str) -> Option<Self> {
            Self::ALL
                .iter()
                .copied()
                .find(|fixture| fixture.name() == name)
        }

        fn name(self) -> &'static str {
            match self {
                Self::StderrProgress => "stderr-progress",
                Self::StderrWarning => "stderr-warning",
                Self::StderrDigits => "stderr-digits",
                Self::StderrOneTwo => "stderr-onetwo",
                Self::StderrOneSleepTwo => "stderr-one-sleep-two",
                Self::StderrFfmpegProgress => "stderr-ffmpeg-progress",
                Self::StderrFfmpegProgressTwice => "stderr-ffmpeg-progress-twice",
                Self::StderrBadnessExit7 => "stderr-badness-exit-7",
                Self::StderrManyLinesExit7 => "stderr-many-lines-exit-7",
                Self::StdoutNoiseStderrFfmpegProgress => "stdout-noise-stderr-ffmpeg-progress",
                Self::StdoutOneSleepTwo => "stdout-one-sleep-two",
                Self::SleepLong => "sleep-long",
                Self::VmafScoreThenSleep => "vmaf-score-then-sleep",
                Self::VmafProgressScore => "vmaf-progress-score",
                Self::VmafNoScore => "vmaf-no-score",
                Self::VmafScoreExit7 => "vmaf-score-exit-7",
                Self::StdoutNoiseVmafProgressScore => "stdout-noise-vmaf-progress-score",
                Self::XpsnrScoreThenSleep => "xpsnr-score-then-sleep",
                Self::XpsnrProgressScore => "xpsnr-progress-score",
                Self::XpsnrNoScore => "xpsnr-no-score",
                Self::XpsnrScoreExit7 => "xpsnr-score-exit-7",
                Self::StdoutNoiseXpsnrProgressScore => "stdout-noise-xpsnr-progress-score",
            }
        }

        fn expected_sequence(self) -> &'static str {
            match self {
                Self::StderrProgress => "stderr: progress; exit 0",
                Self::StderrWarning => "stderr: warning; exit 0",
                Self::StderrDigits => "stderr: 1234567890; exit 0",
                Self::StderrOneTwo => "stderr: onetwo; exit 0",
                Self::StderrOneSleepTwo => "stderr: one; delay; stderr: two; exit 0",
                Self::StderrFfmpegProgress => "stderr: one ffmpeg progress record; exit 0",
                Self::StderrFfmpegProgressTwice => {
                    "stderr: ffmpeg progress frame 12; delay; stderr: ffmpeg progress frame 24; exit 0"
                }
                Self::StderrBadnessExit7 => "stderr: badness; exit 7",
                Self::StderrManyLinesExit7 => "stderr: 5000 numbered lines; exit 7",
                Self::StdoutNoiseStderrFfmpegProgress => {
                    "stdout: noise; stderr: one ffmpeg progress record; exit 0"
                }
                Self::StdoutOneSleepTwo => "stdout: one; delay; stdout: two; exit 0",
                Self::SleepLong => {
                    "delay long enough for timeout/termination tests; exit 0 if not killed"
                }
                Self::VmafScoreThenSleep => {
                    "stderr: VMAF score; delay long enough for cancellation tests"
                }
                Self::VmafProgressScore => {
                    "stderr: ffmpeg progress; delay; stderr: VMAF score; exit 0"
                }
                Self::VmafNoScore => "stderr: ffmpeg progress without score; exit 0",
                Self::VmafScoreExit7 => "stderr: VMAF score; stderr: badness; exit 7",
                Self::StdoutNoiseVmafProgressScore => {
                    "stdout: noise; stderr: ffmpeg progress; delay; stderr: VMAF score; exit 0"
                }
                Self::XpsnrScoreThenSleep => {
                    "stderr: XPSNR score; delay long enough for cancellation tests"
                }
                Self::XpsnrProgressScore => {
                    "stderr: ffmpeg progress; delay; stderr: XPSNR score; exit 0"
                }
                Self::XpsnrNoScore => "stderr: ffmpeg progress without score; exit 0",
                Self::XpsnrScoreExit7 => "stderr: XPSNR score; stderr: badness; exit 7",
                Self::StdoutNoiseXpsnrProgressScore => {
                    "stdout: noise; stderr: ffmpeg progress; delay; stderr: XPSNR score; exit 0"
                }
            }
        }

        fn has_periodic_progress(self) -> bool {
            matches!(self, Self::StderrFfmpegProgressTwice)
        }

        fn has_score_before_continued_runtime(self) -> bool {
            matches!(self, Self::VmafScoreThenSleep | Self::XpsnrScoreThenSleep)
        }

        fn has_noisy_stderr(self) -> bool {
            matches!(self, Self::StderrManyLinesExit7)
        }

        fn has_noisy_stdout(self) -> bool {
            matches!(
                self,
                Self::StdoutNoiseStderrFfmpegProgress
                    | Self::StdoutNoiseVmafProgressScore
                    | Self::StdoutNoiseXpsnrProgressScore
            )
        }

        fn has_non_zero_exit(self) -> bool {
            matches!(
                self,
                Self::StderrBadnessExit7
                    | Self::StderrManyLinesExit7
                    | Self::VmafScoreExit7
                    | Self::XpsnrScoreExit7
            )
        }

        fn has_delayed_eof(self) -> bool {
            matches!(self, Self::StderrOneSleepTwo | Self::StdoutOneSleepTwo)
        }

        fn has_timeout_cleanup(self) -> bool {
            matches!(self, Self::SleepLong)
        }

        fn has_truncation_volume(self) -> bool {
            matches!(self, Self::StderrManyLinesExit7)
        }

        fn supports_delayed_subscription_replay(self) -> bool {
            matches!(
                self,
                Self::StderrOneTwo | Self::VmafProgressScore | Self::XpsnrProgressScore
            )
        }

        fn run(self) {
            match self {
                Self::StderrProgress => eprint!("progress"),
                Self::StderrWarning => eprint!("warning"),
                Self::StderrDigits => eprint!("1234567890"),
                Self::StderrOneTwo => eprint!("onetwo"),
                Self::StderrOneSleepTwo => {
                    eprint!("one");
                    io::stderr().flush().expect("flush stderr");
                    thread::sleep(Duration::from_millis(10));
                    eprint!("two");
                }
                Self::StderrFfmpegProgress => {
                    eprint!(
                        "frame=  12 fps= 24 q=-0.0 size=N/A time=00:00:01.50 bitrate=N/A speed=1x    \r"
                    );
                }
                Self::StderrFfmpegProgressTwice => {
                    eprint!(
                        "frame=  12 fps= 24 q=-0.0 size=N/A time=00:00:01.50 bitrate=N/A speed=1x    \r"
                    );
                    io::stderr().flush().expect("flush stderr");
                    thread::sleep(Duration::from_millis(10));
                    eprint!(
                        "frame=  24 fps= 24 q=-0.0 size=N/A time=00:00:03.00 bitrate=N/A speed=1x    \r"
                    );
                }
                Self::StderrBadnessExit7 => {
                    eprint!("badness");
                    std::process::exit(7);
                }
                Self::StderrManyLinesExit7 => {
                    for n in 0..5000 {
                        eprintln!("line-{n:04}");
                    }
                    std::process::exit(7);
                }
                Self::StdoutNoiseStderrFfmpegProgress => {
                    print!("stdout-noise");
                    eprint!(
                        "frame=  3 fps= 30 q=-0.0 size=N/A time=00:00:00.25 bitrate=N/A speed=1x    \r"
                    );
                }
                Self::StdoutOneSleepTwo => {
                    print!("one");
                    io::stdout().flush().expect("flush stdout");
                    thread::sleep(Duration::from_millis(10));
                    print!("two");
                }
                Self::SleepLong => thread::sleep(Duration::from_secs(30)),
                Self::VmafScoreThenSleep => {
                    eprintln!("VMAF score: 97.500000");
                    thread::sleep(Duration::from_secs(30));
                }
                Self::VmafProgressScore => {
                    eprint!(
                        "frame=  12 fps= 24 q=-0.0 size=N/A time=00:00:01.50 bitrate=N/A speed=1x    \r"
                    );
                    io::stderr().flush().expect("flush stderr");
                    thread::sleep(Duration::from_millis(10));
                    eprintln!("VMAF score: 97.500000");
                }
                Self::VmafNoScore => eprintln!(
                    "frame=  1 fps=  1 q=-0.0 size=N/A time=00:00:00.10 bitrate=N/A speed=1x"
                ),
                Self::VmafScoreExit7 => {
                    eprintln!("VMAF score: 97.500000");
                    eprintln!("vmaf badness");
                    std::process::exit(7);
                }
                Self::StdoutNoiseVmafProgressScore => {
                    print!("stdout-noise");
                    io::stdout().flush().expect("flush stdout");
                    eprint!(
                        "frame=  3 fps= 30 q=-0.0 size=N/A time=00:00:00.25 bitrate=N/A speed=1x    \r"
                    );
                    io::stderr().flush().expect("flush stderr");
                    thread::sleep(Duration::from_millis(10));
                    eprintln!("VMAF score: 98.000000");
                }
                Self::XpsnrScoreThenSleep => {
                    eprintln!(
                        "[Parsed_xpsnr_0 @ 0x1] XPSNR y: 33.6547 u: 41.8741 v: 42.2571 (minimum: 33.6547)"
                    );
                    thread::sleep(Duration::from_secs(30));
                }
                Self::XpsnrProgressScore => {
                    eprint!(
                        "frame=  12 fps= 24 q=-0.0 size=N/A time=00:00:01.50 bitrate=N/A speed=1x    \r"
                    );
                    io::stderr().flush().expect("flush stderr");
                    thread::sleep(Duration::from_millis(10));
                    eprintln!(
                        "[Parsed_xpsnr_0 @ 0x1] XPSNR y: 33.6547 u: 41.8741 v: 42.2571 (minimum: 33.6547)"
                    );
                }
                Self::XpsnrNoScore => eprintln!(
                    "frame=  1 fps=  1 q=-0.0 size=N/A time=00:00:00.10 bitrate=N/A speed=1x"
                ),
                Self::XpsnrScoreExit7 => {
                    eprintln!(
                        "[Parsed_xpsnr_0 @ 0x1] XPSNR y: 33.6547 u: 41.8741 v: 42.2571 (minimum: 33.6547)"
                    );
                    eprintln!("xpsnr badness");
                    std::process::exit(7);
                }
                Self::StdoutNoiseXpsnrProgressScore => {
                    print!("stdout-noise");
                    io::stdout().flush().expect("flush stdout");
                    eprint!(
                        "frame=  3 fps= 30 q=-0.0 size=N/A time=00:00:00.25 bitrate=N/A speed=1x    \r"
                    );
                    io::stderr().flush().expect("flush stderr");
                    thread::sleep(Duration::from_millis(10));
                    eprintln!(
                        "[Parsed_xpsnr_0 @ 0x1] XPSNR y: 34.0000 u: 41.8741 v: 42.2571 (minimum: 34.0000)"
                    );
                }
            }
        }
    }

    #[test]
    fn raw_output_chunk_exposes_borrowed_bytes_for_parsers() {
        let chunk = RawOutputChunk::new(b"progress".to_vec());
        assert_eq!(chunk.as_bytes(), b"progress");
        assert_eq!(chunk.into_bytes(), b"progress");
    }

    #[test]
    fn output_truncation_is_an_explicit_terminal_collection_state() {
        assert_eq!(
            OutputTruncation::from_truncated(false),
            OutputTruncation::Complete
        );
        assert_eq!(
            OutputTruncation::from_truncated(true),
            OutputTruncation::Truncated
        );
    }

    #[test]
    fn managed_process_fixture_child() {
        let Ok(fixture) = env::var(FIXTURE_ENV) else {
            return;
        };

        ManagedProcessFixture::from_name(&fixture)
            .unwrap_or_else(|| panic!("unknown process fixture {fixture}"))
            .run();
    }

    #[test]
    fn streaming_fixture_catalog_covers_required_scenarios() {
        let fixtures = ManagedProcessFixture::ALL;

        for fixture in fixtures {
            assert_eq!(
                ManagedProcessFixture::from_name(fixture.name()),
                Some(*fixture)
            );
            assert!(
                !fixture.expected_sequence().is_empty(),
                "{} should describe its expected stream sequence",
                fixture.name()
            );
        }

        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture.has_periodic_progress())
        );
        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture.has_score_before_continued_runtime())
        );
        assert!(fixtures.iter().any(|fixture| fixture.has_noisy_stderr()));
        assert!(fixtures.iter().any(|fixture| fixture.has_noisy_stdout()));
        assert!(fixtures.iter().any(|fixture| fixture.has_non_zero_exit()));
        assert!(fixtures.iter().any(|fixture| fixture.has_delayed_eof()));
        assert!(fixtures.iter().any(|fixture| fixture.has_timeout_cleanup()));
        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture.has_truncation_volume())
        );
        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture.supports_delayed_subscription_replay())
        );
    }

    async fn assert_score_like_stream_terminates_when_dropped_after_logical_done(
        fixture: &str,
        done_marker: &str,
    ) {
        let cmd = fixture_command(fixture);
        let process =
            ManagedProcess::spawn("score-like stderr fixture", cmd).expect("spawn shell fixture");
        assert!(process.id().is_some(), "process id");
        let mut events = Box::pin(process.terminate_on_drop().stderr_events());
        let mut parsed_logical_done = false;

        while let Some(event) = events.next().await {
            match event.expect("managed event") {
                ManagedEvent::RawStderr(chunk) => {
                    if String::from_utf8_lossy(chunk.as_bytes()).contains(done_marker) {
                        parsed_logical_done = true;
                        break;
                    }
                }
                ManagedEvent::ReplayGap(_) => {}
                ManagedEvent::ProcessDone(_) => {
                    panic!("test must stop polling before ManagedEvent::ProcessDone")
                }
            }
        }

        assert!(parsed_logical_done, "fixture should emit a parseable score");
        drop(events);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    async fn assert_terminate_on_drop_stream_terminates_when_dropped_during_stderr(
        fixture: &str,
        chunk_marker: &str,
    ) {
        let cmd = fixture_command(fixture);
        let process =
            ManagedProcess::spawn("terminate-on-drop fixture", cmd).expect("spawn fixture");
        assert!(process.id().is_some(), "process id");
        let mut events = Box::pin(process.terminate_on_drop().stderr_events());
        let mut saw_chunk = false;

        while let Some(event) = events.next().await {
            match event.expect("managed event") {
                ManagedEvent::RawStderr(chunk) => {
                    if String::from_utf8_lossy(chunk.as_bytes()).contains(chunk_marker) {
                        saw_chunk = true;
                        break;
                    }
                }
                ManagedEvent::ReplayGap(_) => {}
                ManagedEvent::ProcessDone(_) => {
                    panic!("test must stop polling before ManagedEvent::ProcessDone")
                }
            }
        }

        assert!(saw_chunk, "fixture should emit stderr before sleeping");
        drop(events);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn managed_process_collects_stderr_and_waits() {
        let cmd = fixture_command("stderr-progress");

        let (status, stderr) = ManagedProcess::spawn("stderr fixture", cmd)
            .expect("spawn shell fixture")
            .stderr_chunks()
            .await
            .expect("collect stderr");

        assert!(status.success());
        assert_eq!(stderr, b"progress");
    }

    #[tokio::test]
    async fn managed_process_output_returns_status_and_stderr() {
        let cmd = fixture_command("stderr-warning");

        let output = ManagedProcess::spawn("output fixture", cmd)
            .expect("spawn shell fixture")
            .output()
            .await
            .expect("collect output");

        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"warning");
    }

    #[tokio::test]
    async fn managed_process_terminates_after_timeout() {
        let cmd = fixture_command("sleep-long");

        let process = ManagedProcess::spawn("sleep fixture", cmd).expect("spawn shell fixture");
        assert!(
            process.id().is_some(),
            "child should be running before termination"
        );

        let status = process
            .terminate_after(Duration::from_millis(25))
            .await
            .expect("terminate child after timeout");

        assert!(
            !status.success(),
            "terminated process should not exit successfully"
        );
    }

    #[tokio::test]
    #[should_panic]
    async fn dropping_must_complete_process_is_loud() {
        let cmd = fixture_command("sleep-long");

        let process = ManagedProcess::spawn("must-complete fixture", cmd)
            .expect("spawn must-complete fixture")
            .must_complete();

        drop(process);
    }

    #[tokio::test]
    async fn dropping_terminate_on_drop_process_terminates_instead_of_panicking() {
        let cmd = fixture_command("sleep-long");

        let process = ManagedProcess::spawn("terminate-on-drop fixture", cmd)
            .expect("spawn terminate-on-drop fixture")
            .terminate_on_drop();

        drop(process);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn managed_process_output_timeout_returns_error() {
        let cmd = fixture_command("sleep-long");
        let options = ManagedProcessOptions::default().with_wait_timeout(Duration::from_millis(1));

        let err = ManagedProcess::spawn_with_options("timeout fixture", cmd, options)
            .expect("spawn fixture")
            .stderr_output()
            .await
            .expect_err("timeout should be reported as an error");

        assert!(err.to_string().contains("process exceeded"));
    }

    #[tokio::test]
    async fn managed_process_reports_bounded_stderr_truncation() {
        let cmd = fixture_command("stderr-digits");

        let output = ManagedProcess::spawn("noisy stderr fixture", cmd)
            .expect("spawn shell fixture")
            .stderr_output()
            .await
            .expect("collect bounded stderr");

        assert!(output.status.success());
        assert_eq!(output.stderr, b"1234567890");
        assert_eq!(output.stderr_truncation, OutputTruncation::Complete);
        assert!(!output.stderr_truncation.is_truncated());
    }

    #[tokio::test]
    async fn managed_process_reports_custom_bounded_stderr_truncation() {
        let cmd = fixture_command("stderr-digits");
        let options = ManagedProcessOptions::default().with_stderr_limit(4);

        let output = ManagedProcess::spawn_with_options("noisy stderr fixture", cmd, options)
            .expect("spawn shell fixture")
            .stderr_output()
            .await
            .expect("collect bounded stderr");

        assert!(output.status.success());
        assert_eq!(output.stderr, b"7890");
        assert_eq!(output.stderr_truncation, OutputTruncation::Truncated);
        assert!(output.stderr_truncation.is_truncated());
    }

    #[tokio::test]
    async fn managed_process_observes_stderr_chunks_while_waiting() {
        let cmd = fixture_command("stderr-one-sleep-two");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_in_consumer = Arc::clone(&seen);
        let status = ManagedProcess::spawn("streaming stderr fixture", cmd)
            .expect("spawn shell fixture")
            .observe_stderr_chunks(move |chunk| {
                seen_in_consumer
                    .lock()
                    .expect("seen chunks lock")
                    .extend_from_slice(chunk.as_ref());
                Next::Continue
            })
            .await
            .expect("observe stderr");

        assert!(status.success());
        assert!(
            seen.lock()
                .expect("seen chunks lock")
                .windows(b"onetwo".len())
                .any(|window| window == b"onetwo"),
            "stderr observer should include fixture output"
        );
    }

    #[tokio::test]
    async fn managed_process_streams_stderr_events_then_done() {
        let cmd = fixture_command("stderr-onetwo");
        let events = ManagedProcess::spawn("stderr events fixture", cmd)
            .expect("spawn shell fixture")
            .must_complete()
            .stderr_events();
        tokio::pin!(events);

        let mut stderr = Vec::new();
        let mut status = None;
        while let Some(event) = events.next().await {
            match event.expect("managed event") {
                ManagedEvent::RawStderr(chunk) => stderr.extend(chunk.into_bytes()),
                ManagedEvent::ReplayGap(_) => {}
                ManagedEvent::ProcessDone(done) => status = Some(done.status()),
            }
        }

        assert_eq!(stderr, b"onetwo");
        assert!(status.expect("done status").success());
    }

    #[tokio::test]
    async fn managed_process_replays_stderr_emitted_before_subscription() {
        let cmd = fixture_command("stderr-onetwo");
        let process =
            ManagedProcess::spawn("stderr replay fixture", cmd).expect("spawn shell fixture");

        tokio::time::sleep(Duration::from_millis(50)).await;
        let events = process.must_complete().stderr_events();
        tokio::pin!(events);

        let mut stderr = Vec::new();
        while let Some(event) = events.next().await {
            match event.expect("managed event") {
                ManagedEvent::RawStderr(chunk) => stderr.extend(chunk.into_bytes()),
                ManagedEvent::ReplayGap(_) => {}
                ManagedEvent::ProcessDone(_) => break,
            }
        }

        assert_eq!(stderr, b"onetwo");
    }

    #[tokio::test]
    async fn score_like_stderr_event_stream_terminates_when_dropped_after_logical_done() {
        assert_score_like_stream_terminates_when_dropped_after_logical_done(
            "vmaf-score-then-sleep",
            "VMAF score:",
        )
        .await;
        assert_score_like_stream_terminates_when_dropped_after_logical_done(
            "xpsnr-score-then-sleep",
            "XPSNR",
        )
        .await;
    }

    #[tokio::test]
    async fn terminate_on_drop_stderr_event_stream_terminates_when_dropped_during_stderr() {
        assert_terminate_on_drop_stream_terminates_when_dropped_during_stderr(
            "stderr-one-sleep-two",
            "one",
        )
        .await;
    }

    #[tokio::test]
    async fn managed_process_observes_stdout_chunks_while_waiting() {
        let cmd = fixture_command("stdout-one-sleep-two");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_in_consumer = Arc::clone(&seen);
        let status = ManagedProcess::spawn("streaming stdout fixture", cmd)
            .expect("spawn shell fixture")
            .observe_stdout_chunks(move |chunk| {
                seen_in_consumer
                    .lock()
                    .expect("seen chunks lock")
                    .extend_from_slice(chunk.as_ref());
                Next::Continue
            })
            .await
            .expect("observe stdout");

        assert!(status.success());
        assert!(
            seen.lock()
                .expect("seen chunks lock")
                .windows(b"onetwo".len())
                .any(|window| window == b"onetwo"),
            "stdout observer should include fixture output"
        );
    }

    #[tokio::test]
    #[should_panic]
    async fn dropping_live_managed_process_panics_instead_of_silently_detaching() {
        let cmd = fixture_command("sleep-long");

        let process = ManagedProcess::spawn("drop guard fixture", cmd).expect("spawn fixture");

        drop(process);
    }

    #[tokio::test]
    async fn explicit_termination_is_the_supported_active_process_cleanup_path() {
        let cmd = fixture_command("sleep-long");

        let process =
            ManagedProcess::spawn("explicit termination fixture", cmd).expect("spawn fixture");
        assert!(
            process.id().is_some(),
            "managed process should expose liveness before terminal transition"
        );

        let status = process
            .terminate_after(Duration::from_millis(25))
            .await
            .expect("terminate managed process");

        assert!(
            !status.success(),
            "timeout termination should return the child terminal status"
        );
    }
}
