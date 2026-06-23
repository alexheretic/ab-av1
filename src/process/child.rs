use log::info;
use std::{
    future::Future,
    io::IsTerminal,
    mem,
    ops::{Deref, DerefMut},
    pin::{Pin, pin},
    sync::{LazyLock, Mutex},
    time::Duration,
};
use tokio::{
    signal,
    time::{Instant, timeout_at},
};
use tokio_process_stream::ProcessChunkStream;

static RUNNING: LazyLock<Mutex<Vec<ProcessChunkStream>>> = LazyLock::new(<_>::default);

/// Add a child process so it may be waited on before exiting.
pub fn add(mut child: ProcessChunkStream) {
    let mut running = RUNNING.lock().unwrap();

    // remove any that have exited already
    running.retain_mut(|c| {
        c.child_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    });

    if child
        .child_mut()
        .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    {
        running.push(child);
    }
}

/// Wait for all child processes, that were added with [`add`], to exit.
pub async fn wait() {
    // if waiting takes >500ms log what's happening
    let procs = mem::take(&mut *RUNNING.lock().unwrap());
    let mut ctrl_c = pin!(async {
        _ = signal::ctrl_c().await;
    });

    _ = wait_for_children_or_abort(procs, ctrl_c.as_mut()).await;
}

#[derive(Debug, PartialEq)]
enum WaitOutcome {
    Completed,
    Aborted,
}

async fn wait_for_children_or_abort(
    procs: Vec<ProcessChunkStream>,
    mut abort: Pin<&mut impl Future<Output = ()>>,
) -> WaitOutcome {
    let mut log_deadline = Some(Instant::now() + Duration::from_millis(500));

    for mut proc in procs {
        if let Some(child) = proc.child_mut() {
            if let Some(deadline) = log_deadline
                && timeout_at(deadline, child.wait()).await.is_err()
            {
                log_waiting();
                log_deadline = None;
            }
            tokio::select! {
                _ = abort.as_mut() => {
                    log_abort_wait();
                    return WaitOutcome::Aborted;
                }
                _ = child.wait() => {}
            }
        }
    }

    WaitOutcome::Completed
}

fn log_waiting() {
    match std::io::stderr().is_terminal() {
        true => eprintln!("Waiting for child processes to exit..."),
        _ => info!("Waiting for child processes to exit"),
    }
}

fn log_abort_wait() {
    match std::io::stderr().is_terminal() {
        true => eprintln!("Aborting wait for child processes"),
        _ => info!("Aborting wait for child processes"),
    }
}

/// Wrapper that [`add`]s the inner on drop.
#[derive(Debug)]
pub struct AddOnDropChunkStream(Option<ProcessChunkStream>);

impl From<ProcessChunkStream> for AddOnDropChunkStream {
    fn from(v: ProcessChunkStream) -> Self {
        Self(Some(v))
    }
}

impl Drop for AddOnDropChunkStream {
    fn drop(&mut self) {
        if let Some(child) = self.0.take() {
            add(child);
        }
    }
}

impl Deref for AddOnDropChunkStream {
    type Target = ProcessChunkStream;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap() // only none after drop
    }
}

impl DerefMut for AddOnDropChunkStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap() // only none after drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::lock::Mutex as AsyncMutex;
    use std::process::Stdio;
    use tokio::process::Command;

    static TEST_MUTEX: LazyLock<AsyncMutex<()>> = LazyLock::new(<_>::default);

    async fn test_guard() -> futures_util::lock::MutexGuard<'static, ()> {
        TEST_MUTEX.lock().await
    }

    fn clear_running() {
        RUNNING.lock().unwrap().clear();
    }

    fn running_len() -> usize {
        RUNNING.lock().unwrap().len()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_tracks_running_child() {
        let _guard = test_guard().await;
        clear_running();

        let mut child = Command::new("sh");
        child
            .kill_on_drop(true)
            .arg("-c")
            .arg("sleep 1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn running child");

        add(ProcessChunkStream::from(child));

        assert_eq!(running_len(), 1, "running child should be tracked");

        wait().await;
        clear_running();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_ignores_exited_child() {
        let _guard = test_guard().await;
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = child.spawn().expect("spawn exited child");
        child.wait().await.expect("wait exited child");

        add(ProcessChunkStream::from(child));

        assert_eq!(running_len(), 0, "exited child should not be tracked");
        clear_running();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropped_active_chunk_stream_registers_child_for_shutdown() {
        let _guard = test_guard().await;
        clear_running();

        let mut child = Command::new("sh");
        child
            .kill_on_drop(true)
            .arg("-c")
            .arg("sleep 1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn running child");

        let stream = AddOnDropChunkStream::from(ProcessChunkStream::from(child));
        drop(stream);

        assert_eq!(
            running_len(),
            1,
            "dropped active stream should transfer child to shutdown tracking"
        );

        wait().await;
        clear_running();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn interrupted_wait_aborts_without_waiting_for_running_child() {
        let _guard = test_guard().await;
        clear_running();

        let mut child = Command::new("sh");
        child
            .kill_on_drop(true)
            .arg("-c")
            .arg("sleep 30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn running child");
        add(ProcessChunkStream::from(child));

        let procs = mem::take(&mut *RUNNING.lock().unwrap());
        let mut interrupt = pin!(std::future::ready(()));

        assert_eq!(
            wait_for_children_or_abort(procs, interrupt.as_mut()).await,
            WaitOutcome::Aborted,
            "interrupt path should stop waiting for children"
        );
        assert_eq!(
            running_len(),
            0,
            "interrupted wait does not re-register children in current behavior"
        );

        clear_running();
    }
}
