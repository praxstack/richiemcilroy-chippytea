//! Bounded, read-only subprocess supervision. A timed-out or cancelled caller
//! never waits for a stuck child to reap. The worker keeps its slot until reaping
//! succeeds: a stuck kernel call cannot cause an unbounded replacement pool.
use crate::model::Result;
use std::{
    io::Read,
    os::unix::{io::AsRawFd, process::CommandExt},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

const ACTIVE_PROBES: usize = 2;
const QUEUED_PROBES: usize = 2;
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(10);

struct Request {
    command: Command,
    deadline: Instant,
    output_limit: usize,
    aborted: Arc<AtomicBool>,
    reply: SyncSender<Result<Output>>,
}

struct Pool {
    requests: SyncSender<Request>,
}

impl Pool {
    fn new(active: usize, queued: usize) -> Self {
        let (requests, receive) = mpsc::sync_channel::<Request>(queued);
        let receive = Arc::new(Mutex::new(receive));
        for _ in 0..active {
            let receive = Arc::clone(&receive);
            let _started = std::thread::Builder::new()
                .name("chippytea-read-only-probe".into())
                .spawn(move || {
                    loop {
                        let request = receive.lock().unwrap().recv();
                        match request {
                            Ok(request) => supervise(request),
                            Err(_) => return,
                        }
                    }
                });
            // Resource exhaustion must fail closed through the disconnected
            // or deadline-bounded request channel, never panic in accounting.
        }
        Self { requests }
    }

    fn run(
        &self,
        command: Command,
        timeout: Duration,
        output_limit: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<Output> {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
            return Err("Read-only probe cancelled".into());
        }
        if !Path::new(command.get_program()).is_absolute()
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
            || output_limit == 0
            || output_limit > MAX_OUTPUT
            || command
                .get_args()
                .map(|argument| argument.as_encoded_bytes().len())
                .sum::<usize>()
                > MAX_ARGUMENT_BYTES
        {
            return Err("Invalid read-only probe limits or executable".into());
        }
        let deadline = Instant::now() + timeout;
        let aborted = Arc::new(AtomicBool::new(false));
        let (reply, receive) = mpsc::sync_channel(1);
        match self.requests.try_send(Request {
            command,
            deadline,
            output_limit,
            aborted: Arc::clone(&aborted),
            reply,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Err("Read-only probe capacity is busy; try again later".into());
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err("Read-only probe supervisors are unavailable".into());
            }
        }
        loop {
            let reason = if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
                Some("Read-only probe cancelled")
            } else if Instant::now() >= deadline {
                Some("Read-only probe timed out")
            } else {
                None
            };
            if let Some(reason) = reason {
                aborted.store(true, Ordering::Release);
                return Err(reason.into());
            }
            match receive.recv_timeout(POLL.min(deadline.saturating_duration_since(Instant::now())))
            {
                Ok(output) => return output,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Read-only probe supervisor ended without a result".into());
                }
            }
        }
    }
}

/// Callers construct a fixed, trusted executable and argument list, never a
/// shell command derived from a scanned manifest or a frontend request. This
/// facility is intentionally not used for cleanup/mutating owner commands.
pub(crate) fn run(
    command: Command,
    timeout: Duration,
    output_limit: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Output> {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(ACTIVE_PROBES, QUEUED_PROBES))
        .run(command, timeout, output_limit, cancel)
}

fn supervise(mut request: Request) {
    if request.aborted.load(Ordering::Acquire) || Instant::now() >= request.deadline {
        let _ = request
            .reply
            .try_send(Err("Read-only probe timed out or cancelled".into()));
        return;
    }
    request
        .command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // Spawn itself may block in the OS. The caller still has a deadline, and
    // this worker remains occupied instead of spawning a replacement thread.
    let mut child = match request.command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = request
                .reply
                .try_send(Err(format!("Read-only probe could not start: {error}")));
            return;
        }
    };
    let outcome = collect(&mut child, &request);
    let failed = outcome.is_err();
    let _ = request.reply.try_send(outcome);
    if failed {
        // Do not reap the leader until both pipes reach EOF on the success
        // path. Until here its PID cannot be reused, making group cancellation
        // safe even when a child inherited the output pipes.
        unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
        let _ = child.kill();
        child.stdout.take();
        child.stderr.take();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Err(error) if error.raw_os_error() == Some(libc::ECHILD) => return,
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

fn collect(child: &mut Child, request: &Request) -> Result<Output> {
    for fd in [
        child.stdout.as_ref().unwrap().as_raw_fd(),
        child.stderr.as_ref().unwrap().as_raw_fd(),
    ] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err("Read-only probe output could not be made nonblocking".into());
        }
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    loop {
        if request.aborted.load(Ordering::Acquire) {
            return Err("Read-only probe cancelled".into());
        }
        if Instant::now() >= request.deadline {
            return Err("Read-only probe timed out".into());
        }
        if !stdout_done {
            stdout_done = drain(
                child.stdout.as_mut().unwrap(),
                &mut stdout,
                request.output_limit.saturating_sub(stderr.len()),
                request,
            )?;
        }
        if !stderr_done {
            stderr_done = drain(
                child.stderr.as_mut().unwrap(),
                &mut stderr,
                request.output_limit.saturating_sub(stdout.len()),
                request,
            )?;
        }
        if stdout_done && stderr_done {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(Output {
                        status,
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(format!("Read-only probe status is unavailable: {error}"));
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

fn drain(
    pipe: &mut impl Read,
    bytes: &mut Vec<u8>,
    limit: usize,
    request: &Request,
) -> Result<bool> {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        if request.aborted.load(Ordering::Acquire) || Instant::now() >= request.deadline {
            return Err("Read-only probe timed out or cancelled".into());
        }
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) if count > limit.saturating_sub(bytes.len()) => {
                return Err("Read-only probe exceeded its output limit".into());
            }
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Read-only probe output was incomplete: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn captures_both_streams_and_the_exact_exit_status() {
        let pool = Pool::new(1, 1);
        let output = pool
            .run(
                shell("printf output; printf warning >&2; exit 7"),
                Duration::from_secs(2),
                1024,
                None,
            )
            .unwrap();
        assert_eq!(output.stdout, b"output");
        assert_eq!(output.stderr, b"warning");
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn output_limit_applies_to_both_streams_together() {
        let pool = Pool::new(1, 1);
        let error = pool
            .run(
                shell("printf 12345; printf 67890 >&2"),
                Duration::from_secs(2),
                8,
                None,
            )
            .unwrap_err();
        assert!(error.contains("output limit"), "{error}");
    }

    #[test]
    fn deadline_is_bounded_even_when_a_descendant_holds_the_pipe() {
        let pool = Pool::new(1, 1);
        let started = Instant::now();
        let error = pool
            .run(
                shell("sleep 10 & exit 0"),
                Duration::from_millis(80),
                1024,
                None,
            )
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cancellation_does_not_wait_for_the_child() {
        let pool = Pool::new(1, 1);
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(40));
                cancel.store(true, Ordering::Release);
            });
            let started = Instant::now();
            let error = pool
                .run(
                    shell("sleep 10"),
                    Duration::from_secs(2),
                    1024,
                    Some(&cancel),
                )
                .unwrap_err();
            assert!(error.contains("cancelled"), "{error}");
            assert!(started.elapsed() < Duration::from_secs(1));
        });
    }

    #[test]
    fn admission_never_grows_a_queue_behind_a_busy_worker() {
        let pool = Pool::new(1, 1);
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let mut command = shell("printf ready > \"$1\"; sleep 10");
        command.arg("probe-fixture").arg(&ready);
        std::thread::scope(|scope| {
            scope.spawn(|| pool.run(command, Duration::from_millis(300), 1024, None));
            let deadline = Instant::now() + Duration::from_secs(2);
            while !ready.exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let (reply, _) = mpsc::sync_channel(1);
            pool.requests
                .try_send(Request {
                    command: shell("exit 0"),
                    deadline: Instant::now(),
                    output_limit: 1024,
                    aborted: Arc::new(AtomicBool::new(false)),
                    reply,
                })
                .unwrap();
            let started = Instant::now();
            let error = pool
                .run(shell("exit 0"), Duration::from_secs(2), 1024, None)
                .unwrap_err();
            assert!(error.contains("capacity is busy"), "{error}");
            assert!(started.elapsed() < Duration::from_millis(100));
        });
    }
}
