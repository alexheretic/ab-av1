#![allow(
    dead_code,
    reason = "managed process wrapper is introduced before callers migrate onto it"
)]

use std::process::{ExitStatus, Output};
use std::time::Duration;
use tokio::process::Command;
use tokio_process_tools::{Chunk, visitors::inspect::InspectChunks};
use tokio_process_tools::{
    CollectionOverflowBehavior, Consumable, DEFAULT_MAX_BUFFERED_CHUNKS,
    DEFAULT_OUTPUT_EOF_TIMEOUT, DEFAULT_READ_CHUNK_SIZE, GracefulShutdown, Next, Process,
    RawCollectionOptions, RawOutputOptions, StreamEvent, Subscribable, Subscription,
};
use tokio_process_tools::{NumBytesExt, ProcessHandle, SingleSubscriberOutputStream};
use tokio_stream::Stream;

const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
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
    pub fn with_stderr_limit(mut self, stderr_limit: usize) -> Self {
        self.stderr_limit = stderr_limit;
        self
    }
}

pub struct ManagedProcess {
    handle: ProcessHandle<SingleSubscriberOutputStream, SingleSubscriberOutputStream>,
    options: ManagedProcessOptions,
}

pub struct ManagedOutput {
    pub status: ExitStatus,
    pub stderr: Vec<u8>,
    pub stderr_truncated: bool,
}

pub enum ManagedEvent {
    Stderr(Vec<u8>),
    Done(ExitStatus),
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
                    .no_replay()
                    .read_chunk_size(DEFAULT_READ_CHUNK_SIZE)
                    .max_buffered_chunks(DEFAULT_MAX_BUFFERED_CHUNKS)
            })
            .spawn()?;
        Ok(Self { handle, options })
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
        let output = self
            .handle
            .wait_for_completion(self.options.wait_timeout)
            .with_raw_output(
                DEFAULT_OUTPUT_EOF_TIMEOUT,
                RawOutputOptions::symmetric(RawCollectionOptions::Bounded {
                    max_bytes: self.options.stderr_limit.bytes(),
                    overflow_behavior: CollectionOverflowBehavior::DropOldestData,
                }),
            )
            .await?
            .expect_completed("process should complete");

        Ok(ManagedOutput {
            status: output.status,
            stderr: output.stderr.bytes,
            stderr_truncated: output.stderr.truncated,
        })
    }

    pub async fn observe_stderr_chunks(
        mut self,
        on_chunk: impl FnMut(Chunk) -> Next + Send + 'static,
    ) -> anyhow::Result<ExitStatus> {
        let consumer = self
            .handle
            .stderr()
            .consume(InspectChunks::builder().f(on_chunk).build())?;
        let status = self
            .handle
            .wait_for_completion(self.options.wait_timeout)
            .await?
            .expect_completed("process should complete");
        consumer.wait().await?;
        Ok(status)
    }

    pub fn stderr_events(mut self) -> impl Stream<Item = anyhow::Result<ManagedEvent>> {
        async_stream::try_stream! {
            let mut stderr = self.handle.stderr().try_subscribe()?;
            while let Some(event) = stderr.next_event().await {
                match event {
                    StreamEvent::Chunk(chunk) => {
                        yield ManagedEvent::Stderr(chunk.as_ref().to_vec());
                    }
                    StreamEvent::Gap => {}
                    StreamEvent::Eof => break,
                    StreamEvent::ReadError(err) => Err(err)?,
                }
            }

            let status = self
                .handle
                .wait_for_completion(self.options.wait_timeout)
                .await?
                .expect_completed("process should complete");
            yield ManagedEvent::Done(status);
        }
    }

    pub fn stderr_events_terminate_on_drop(
        self,
    ) -> impl Stream<Item = anyhow::Result<ManagedEvent>> {
        struct TerminateOnDrop(Option<ManagedProcess>);

        impl Drop for TerminateOnDrop {
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

        async_stream::try_stream! {
            let mut process = TerminateOnDrop(Some(self));
            let Some(inner) = process.0.as_mut() else {
                return;
            };
            let mut stderr = inner.handle.stderr().try_subscribe()?;
            while let Some(event) = stderr.next_event().await {
                match event {
                    StreamEvent::Chunk(chunk) => {
                        yield ManagedEvent::Stderr(chunk.as_ref().to_vec());
                    }
                    StreamEvent::Gap => {}
                    StreamEvent::Eof => break,
                    StreamEvent::ReadError(err) => Err(err)?,
                }
            }

            let Some(mut inner) = process.0.take() else {
                return;
            };
            let status = inner
                .handle
                .wait_for_completion(inner.options.wait_timeout)
                .await?
                .expect_completed("process should complete");
            yield ManagedEvent::Done(status);
        }
    }

    pub async fn observe_stdout_chunks(
        mut self,
        on_chunk: impl FnMut(Chunk) -> Next + Send + 'static,
    ) -> anyhow::Result<ExitStatus> {
        let consumer = self
            .handle
            .stdout()
            .consume(InspectChunks::builder().f(on_chunk).build())?;
        let status = self
            .handle
            .wait_for_completion(self.options.wait_timeout)
            .await?
            .expect_completed("process should complete");
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

    #[test]
    fn managed_process_fixture_child() {
        let Ok(fixture) = env::var(FIXTURE_ENV) else {
            return;
        };

        match fixture.as_str() {
            "stderr-progress" => eprint!("progress"),
            "stderr-warning" => eprint!("warning"),
            "stderr-digits" => eprint!("1234567890"),
            "stderr-onetwo" => eprint!("onetwo"),
            "stderr-one-sleep-two" => {
                eprint!("one");
                io::stderr().flush().expect("flush stderr");
                thread::sleep(Duration::from_millis(10));
                eprint!("two");
            }
            "stderr-ffmpeg-progress" => {
                eprint!(
                    "frame=  12 fps= 24 q=-0.0 size=N/A time=00:00:01.50 bitrate=N/A speed=1x    \r"
                );
            }
            "stderr-badness-exit-7" => {
                eprint!("badness");
                std::process::exit(7);
            }
            "stdout-noise-stderr-ffmpeg-progress" => {
                print!("stdout-noise");
                eprint!(
                    "frame=  3 fps= 30 q=-0.0 size=N/A time=00:00:00.25 bitrate=N/A speed=1x    \r"
                );
            }
            "stdout-one-sleep-two" => {
                print!("one");
                io::stdout().flush().expect("flush stdout");
                thread::sleep(Duration::from_millis(10));
                print!("two");
            }
            "sleep-long" => thread::sleep(Duration::from_secs(30)),
            "vmaf-score-then-sleep" => {
                eprintln!("VMAF score: 97.500000");
                thread::sleep(Duration::from_secs(30));
            }
            "xpsnr-score-then-sleep" => {
                eprintln!(
                    "[Parsed_xpsnr_0 @ 0x1] XPSNR y: 33.6547 u: 41.8741 v: 42.2571 (minimum: 33.6547)"
                );
                thread::sleep(Duration::from_secs(30));
            }
            other => panic!("unknown process fixture {other}"),
        }
    }

    async fn assert_score_like_stream_terminates_when_dropped_after_logical_done(
        fixture: &str,
        done_marker: &str,
    ) {
        let cmd = fixture_command(fixture);
        let process =
            ManagedProcess::spawn("score-like stderr fixture", cmd).expect("spawn shell fixture");
        assert!(process.id().is_some(), "process id");
        let mut events = Box::pin(process.stderr_events_terminate_on_drop());
        let mut parsed_logical_done = false;

        while let Some(event) = events.next().await {
            match event.expect("managed event") {
                ManagedEvent::Stderr(chunk) => {
                    if String::from_utf8_lossy(&chunk).contains(done_marker) {
                        parsed_logical_done = true;
                        break;
                    }
                }
                ManagedEvent::Done(_) => panic!("test must stop polling before ManagedEvent::Done"),
            }
        }

        assert!(parsed_logical_done, "fixture should emit a parseable score");
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
    async fn managed_process_reports_bounded_stderr_truncation() {
        let cmd = fixture_command("stderr-digits");

        let output = ManagedProcess::spawn("noisy stderr fixture", cmd)
            .expect("spawn shell fixture")
            .stderr_output()
            .await
            .expect("collect bounded stderr");

        assert!(output.status.success());
        assert_eq!(output.stderr, b"1234567890");
        assert!(!output.stderr_truncated);
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
        assert!(output.stderr_truncated);
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
            "stdout observer should include fixture output"
        );
    }

    #[tokio::test]
    async fn managed_process_streams_stderr_events_then_done() {
        let cmd = fixture_command("stderr-onetwo");
        let events = ManagedProcess::spawn("stderr events fixture", cmd)
            .expect("spawn shell fixture")
            .stderr_events();
        tokio::pin!(events);

        let mut stderr = Vec::new();
        let mut status = None;
        while let Some(event) = events.next().await {
            match event.expect("managed event") {
                ManagedEvent::Stderr(chunk) => stderr.extend(chunk),
                ManagedEvent::Done(done) => status = Some(done),
            }
        }

        assert_eq!(stderr, b"onetwo");
        assert!(status.expect("done status").success());
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
