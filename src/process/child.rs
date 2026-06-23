use log::info;
use std::{
    io::IsTerminal,
    mem,
    ops::{Deref, DerefMut},
    pin::pin,
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
    let mut log_deadline = Some(Instant::now() + Duration::from_millis(500));
    let procs = mem::take(&mut *RUNNING.lock().unwrap());
    let mut ctrl_c = pin!(signal::ctrl_c());

    for mut proc in procs {
        if let Some(child) = proc.child_mut() {
            if let Some(deadline) = log_deadline
                && timeout_at(deadline, child.wait()).await.is_err()
            {
                log_waiting();
                log_deadline = None;
            }
            tokio::select! {
                _ = &mut ctrl_c => {
                    log_abort_wait();
                    return;
                }
                _ = child.wait() => {}
            }
        }
    }
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
    use std::{process::Stdio, sync::MutexGuard};
    use tokio::process::Command;

    static TEST_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(<_>::default);

    fn test_guard() -> MutexGuard<'static, ()> {
        TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_running() {
        RUNNING.lock().unwrap().clear();
    }

    fn running_len() -> usize {
        RUNNING.lock().unwrap().len()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_tracks_running_child() {
        let _guard = test_guard();
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("sleep 5")
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
        let _guard = test_guard();
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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drop_of_running_stream_registers_child() {
        let _guard = test_guard();
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn running child");

        let tracked = AddOnDropChunkStream::from(ProcessChunkStream::from(child));
        drop(tracked);

        assert_eq!(
            running_len(),
            1,
            "dropping a live stream should register it"
        );

        wait().await;
        clear_running();
    }
}
