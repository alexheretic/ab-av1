use log::info;
use std::{
    io::IsTerminal,
    mem,
    pin::{Pin, pin},
    sync::{LazyLock, Mutex, MutexGuard},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    signal,
    time::{Instant, timeout_at},
};
use tokio_process_stream::ProcessChunkStream;
use tokio_stream::Stream;

static RUNNING: LazyLock<Mutex<Vec<ProcessChunkStream>>> = LazyLock::new(<_>::default);

fn running_registry() -> MutexGuard<'static, Vec<ProcessChunkStream>> {
    RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn is_running(proc: &mut ProcessChunkStream) -> bool {
    proc.child_mut()
        .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
}

fn prune_exited(running: &mut Vec<ProcessChunkStream>) {
    running.retain_mut(is_running);
}

fn register_if_running(running: &mut Vec<ProcessChunkStream>, mut child: ProcessChunkStream) {
    if is_running(&mut child) {
        running.push(child);
    }
}

struct RegisteredChildren(Vec<ProcessChunkStream>);

impl RegisteredChildren {
    fn take() -> Self {
        Self(mem::take(&mut *running_registry()))
    }

    fn as_mut_slice(&mut self) -> &mut [ProcessChunkStream] {
        &mut self.0
    }

    async fn kill_and_reap_all(&mut self) {
        for proc in &mut self.0 {
            if let Some(child) = proc.child_mut()
                && child.try_wait().is_ok_and(|status| status.is_none())
            {
                _ = child.kill().await;
            }
        }
    }

    fn discard(mut self) {
        self.0.clear();
    }
}

impl Drop for RegisteredChildren {
    fn drop(&mut self) {
        if self.0.is_empty() {
            return;
        }

        let mut running = running_registry();
        prune_exited(&mut running);
        for child in self.0.drain(..) {
            register_if_running(&mut running, child);
        }
    }
}

/// Add a child process so it may be waited on before exiting.
pub fn add(child: ProcessChunkStream) {
    let mut running = running_registry();

    // remove any that have exited already
    prune_exited(&mut running);
    register_if_running(&mut running, child);
}

/// Wait for all child processes, that were added with [`add`], to exit.
pub async fn wait() {
    let mut procs = RegisteredChildren::take();
    let mut ctrl_c = pin!(signal::ctrl_c());

    tokio::select! {
        _ = &mut ctrl_c => {
            log_abort_wait();
            procs.kill_and_reap_all().await;
            procs.discard();
        }
        _ = wait_for_children(
            procs.as_mut_slice(),
            Duration::from_millis(500),
            log_waiting,
        ) => {}
    }
}

async fn wait_for_children(
    procs: &mut [ProcessChunkStream],
    slow_wait_after: Duration,
    mut on_slow_wait: impl FnMut(),
) {
    let mut log_deadline = Some(Instant::now() + slow_wait_after);

    for proc in procs {
        if let Some(child) = proc.child_mut() {
            if let Some(deadline) = log_deadline
                && timeout_at(deadline, child.wait()).await.is_err()
            {
                on_slow_wait();
                log_deadline = None;
            }
            let _ = child.wait().await;
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

/// Stream wrapper that [`add`]s the inner child process on drop.
#[derive(Debug)]
pub struct TrackedChildStream(Option<ProcessChunkStream>);

impl From<ProcessChunkStream> for TrackedChildStream {
    fn from(v: ProcessChunkStream) -> Self {
        Self(Some(v))
    }
}

impl Drop for TrackedChildStream {
    fn drop(&mut self) {
        if let Some(child) = self.0.take() {
            add(child);
        }
    }
}

impl Stream for TrackedChildStream {
    type Item = <ProcessChunkStream as Stream>::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.0.as_mut() {
            Some(stream) => Pin::new(stream).poll_next(cx),
            None => Poll::Ready(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Stdio, sync::MutexGuard};
    use tokio::process::Command;
    use tokio_process_stream::Item;
    use tokio_stream::StreamExt;

    static TEST_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(<_>::default);

    fn test_guard() -> MutexGuard<'static, ()> {
        TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_running() {
        running_registry().clear();
    }

    fn running_len() -> usize {
        running_registry().len()
    }

    fn all_children_reaped(registered: &mut RegisteredChildren) -> bool {
        registered
            .as_mut_slice()
            .iter_mut()
            .all(|proc| !is_running(proc))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_tracks_running_child() {
        let _guard = test_guard();
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
            .arg("sleep 1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn running child");

        let tracked = TrackedChildStream::from(ProcessChunkStream::from(child));
        drop(tracked);

        assert_eq!(
            running_len(),
            1,
            "dropping a live stream should register it"
        );

        wait().await;
        clear_running();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tracked_child_stream_yields_inner_process_items() {
        let _guard = test_guard();
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("printf tracked-output")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn output child");
        let mut tracked = TrackedChildStream::from(ProcessChunkStream::from(child));
        let mut stdout = Vec::new();

        while let Some(next) = tracked.next().await {
            match next {
                Item::Stdout(chunk) => stdout.extend_from_slice(&chunk),
                Item::Stderr(_) => {}
                Item::Done(status) => {
                    assert!(status.expect("wait output child").success());
                    break;
                }
            }
        }
        drop(tracked);

        assert_eq!(stdout, b"tracked-output");
        assert_eq!(
            running_len(),
            0,
            "a completed tracked stream should not be registered on drop"
        );

        clear_running();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_wait_keeps_unfinished_children_registered() {
        let _guard = test_guard();
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("sleep 0.2")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn running child");

        add(ProcessChunkStream::from(child));

        let timed_out = tokio::time::timeout(Duration::from_millis(10), wait())
            .await
            .is_err();
        let registered_after_cancel = running_len();

        if registered_after_cancel > 0 {
            wait().await;
        } else {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        clear_running();

        assert!(timed_out, "wait should still be pending for a live child");
        assert_eq!(
            registered_after_cancel, 1,
            "cancelling wait should not drop unfinished children from the shutdown registry"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn aborting_shutdown_wait_kills_reaps_and_discards_unfinished_children() {
        let _guard = test_guard();
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
        let mut registered = RegisteredChildren::take();
        registered.kill_and_reap_all().await;
        let killed_and_reaped = all_children_reaped(&mut registered);
        registered.discard();

        assert_eq!(
            running_len(),
            0,
            "aborted shutdown children should not be re-registered"
        );
        assert!(
            killed_and_reaped,
            "aborted shutdown children should be killed and reaped before discard"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn aborting_shutdown_wait_reaps_children_that_exit_before_cleanup() {
        let _guard = test_guard();
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("sleep 0.05")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn short-lived child");

        add(ProcessChunkStream::from(child));
        tokio::time::sleep(Duration::from_millis(150)).await;

        let mut registered = RegisteredChildren::take();
        registered.kill_and_reap_all().await;
        let reaped = all_children_reaped(&mut registered);
        registered.discard();

        assert_eq!(
            running_len(),
            0,
            "aborted shutdown children should not be re-registered after exit races"
        );
        assert!(
            reaped,
            "children that exit before abort cleanup should still be reaped before discard"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_prunes_children_that_already_exited() {
        let _guard = test_guard();
        clear_running();

        let mut exited = Command::new("sh");
        exited
            .arg("-c")
            .arg("sleep 0.1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let exited = exited.spawn().expect("spawn short-lived child");

        add(ProcessChunkStream::from(exited));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let mut running = Command::new("sh");
        running
            .arg("-c")
            .arg("sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let running = running.spawn().expect("spawn running child");

        add(ProcessChunkStream::from(running));

        assert_eq!(
            running_len(),
            1,
            "adding a child should prune any tracked children that already exited"
        );

        wait().await;
        clear_running();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_for_children_waits_for_supplied_children() {
        let _guard = test_guard();
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("sleep 0.1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn short-lived child");
        let mut children = vec![ProcessChunkStream::from(child)];
        let mut slow_wait_logged = false;

        wait_for_children(&mut children, Duration::from_secs(60), || {
            slow_wait_logged = true
        })
        .await;

        assert!(!slow_wait_logged, "short waits should not log");
        assert!(
            children.iter_mut().all(|child| !is_running(child)),
            "wait_for_children should leave supplied children exited"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_for_children_logs_once_after_slow_wait_threshold() {
        let _guard = test_guard();
        clear_running();

        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg("sleep 0.1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = child.spawn().expect("spawn slow child");
        let mut children = vec![ProcessChunkStream::from(child)];
        let mut slow_wait_logs = 0;

        wait_for_children(&mut children, Duration::from_millis(10), || {
            slow_wait_logs += 1
        })
        .await;

        assert_eq!(slow_wait_logs, 1, "slow wait should log once");
    }
}
