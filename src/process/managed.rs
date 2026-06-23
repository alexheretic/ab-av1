#![allow(
    dead_code,
    reason = "tokio-process-tools wrapper is introduced before callers migrate onto it"
)]

use std::process::ExitStatus;
use std::time::Duration;
use tokio::process::Command;
use tokio_process_tools::{Chunk, visitors::inspect::InspectChunks};
use tokio_process_tools::{
    CollectionOverflowBehavior, Consumable, DEFAULT_MAX_BUFFERED_CHUNKS,
    DEFAULT_OUTPUT_EOF_TIMEOUT, DEFAULT_READ_CHUNK_SIZE, GracefulShutdown, Next, Process,
    RawCollectionOptions, RawOutputOptions,
};
use tokio_process_tools::{NumBytesExt, ProcessHandle, SingleSubscriberOutputStream};

pub struct ManagedProcess {
    handle: ProcessHandle<SingleSubscriberOutputStream, SingleSubscriberOutputStream>,
}

pub struct ManagedOutput {
    pub status: ExitStatus,
    pub stderr: Vec<u8>,
    pub stderr_truncated: bool,
}

impl ManagedProcess {
    pub fn spawn(name: &'static str, cmd: Command) -> anyhow::Result<Self> {
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
        Ok(Self { handle })
    }

    pub async fn stderr_chunks(self) -> anyhow::Result<(ExitStatus, Vec<u8>)> {
        let output = self.stderr_output(32_768).await?;
        Ok((output.status, output.stderr))
    }

    pub async fn stderr_output(mut self, max_bytes: usize) -> anyhow::Result<ManagedOutput> {
        let output = self
            .handle
            .wait_for_completion(Duration::from_secs(30))
            .with_raw_output(
                DEFAULT_OUTPUT_EOF_TIMEOUT,
                RawOutputOptions::symmetric(RawCollectionOptions::Bounded {
                    max_bytes: max_bytes.bytes(),
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
            .wait_for_completion(Duration::from_secs(30))
            .await?
            .expect_completed("process should complete");
        consumer.wait().await?;
        Ok(status)
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
            .wait_for_completion(Duration::from_secs(30))
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
                    .unix_sigterm(Duration::from_millis(25))
                    .windows_ctrl_break(Duration::from_millis(25))
                    .build(),
            )
            .await?
            .into_result())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::process::Command;

    #[tokio::test]
    async fn managed_process_collects_stderr_and_waits() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf progress >&2");

        let (status, stderr) = ManagedProcess::spawn("stderr fixture", cmd)
            .expect("spawn shell fixture")
            .stderr_chunks()
            .await
            .expect("collect stderr");

        assert!(status.success());
        assert_eq!(stderr, b"progress");
    }

    #[tokio::test]
    async fn managed_process_terminates_after_timeout() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 30");

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
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf 1234567890 >&2");

        let output = ManagedProcess::spawn("noisy stderr fixture", cmd)
            .expect("spawn shell fixture")
            .stderr_output(4)
            .await
            .expect("collect bounded stderr");

        assert!(output.status.success());
        assert_eq!(output.stderr, b"7890");
        assert!(output.stderr_truncated);
    }

    #[tokio::test]
    async fn managed_process_observes_stderr_chunks_while_waiting() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("printf one >&2; sleep 0.01; printf two >&2");

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
        assert_eq!(&*seen.lock().expect("seen chunks lock"), b"onetwo");
    }

    #[tokio::test]
    async fn managed_process_observes_stdout_chunks_while_waiting() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf one; sleep 0.01; printf two");

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
        assert_eq!(&*seen.lock().expect("seen chunks lock"), b"onetwo");
    }
}
