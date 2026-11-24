//! One bounded command in the student's worktree: its own process group,
//! output drained into a bounded tail, and the whole group killed
//! when it outlives its limit. Not a sandbox: the commands are the checkout's
//! own build tools, run with the student's permissions.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The bytes of output a run keeps, from its end.
pub const TAIL_BYTES: usize = 64 * 1024;

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// It exited with this status.
    Exited(i32),
    /// A signal ended it before it exited.
    Killed,
    /// It outlived its limit and its process group was killed.
    Timeout,
    /// It could not be started.
    NotStarted(String),
}

#[derive(Debug, Clone)]
pub struct Ran {
    pub ended: Ended,
    /// The last `TAIL_BYTES` of stdout and stderr together, lossily decoded.
    pub tail: String,
}

/// The course Makefile's variables that choose what is built.
const SELECTORS: [&str; 2] = ["MUTANT", "VARIANT"];

/// Run `program args` in `dir` with `env` added, and the course's build
/// selectors removed, for at most `limit`.
pub fn run(
    program: &str,
    args: &[String],
    dir: &Path,
    env: &[(String, String)],
    limit: Duration,
) -> Ran {
    let started = Instant::now();
    let not_started = |why: String| Ran {
        ended: Ended::NotStarted(why),
        tail: String::new(),
    };
    let Some(resolved) = resolve(program, dir) else {
        return not_started(format!("{program}: not found"));
    };
    let (mut output, writer) = match std::io::pipe() {
        Ok(pair) => pair,
        Err(error) => return not_started(format!("no output pipe: {error}")),
    };
    let stdout = match writer.try_clone() {
        Ok(writer) => writer,
        Err(error) => return not_started(format!("no output pipe handle: {error}")),
    };

    // The process group outlives CodeTrial if CodeTrial is killed, so each
    // command also carries its own limit, which coreutils' `timeout` keeps
    // whether or not anyone is left to kill it.
    let mut command = match resolve("timeout", dir) {
        Some(timeout) => {
            let mut command = Command::new(timeout);
            command
                .arg("--kill-after=10")
                .arg(format!("{}s", limit.as_secs().saturating_add(5)))
                .arg(&resolved);
            command
        }
        None => Command::new(&resolved),
    };

    // The course's build selectors reach a command only as the attempt names
    // them, on make's command line: one exported where CodeTrial runs would
    // make the clean model a mutant, or another variant.
    for selector in SELECTORS {
        command.env_remove(selector);
    }
    command
        .args(args)
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(writer));

    // Its own process group, so the group can be killed whole. Under coreutils'
    // `timeout`, which makes a group of its own, this changes nothing; without
    // it, it is what keeps a leftover process killable.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return not_started(format!("{program}: {error}")),
    };
    drop(command);

    // The tail is shared with the reader, so what arrived is kept even when a
    // process that left the group still holds the pipe and no end comes.
    let tail = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let reading = std::sync::Arc::clone(&tail);
    std::thread::spawn(move || {
        read_tail(&mut output, &reading);
        let _sent = sender.send(());
    });
    let ended = wait_bounded(&mut child, started + limit);
    // Whatever it left running, a daemon or a test runner's fork, goes too.
    kill_group(&mut child);
    let _ended = receiver.recv_timeout(Duration::from_secs(1));
    let bytes: Vec<u8> = tail
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .copied()
        .collect();
    Ran {
        ended,
        tail: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// The executable `program` names: a path, relative to `dir` where the
/// command runs, or a bare name on `PATH`. The path found is the one run.
fn resolve(program: &str, dir: &Path) -> Option<std::path::PathBuf> {
    let named = Path::new(program);
    if named.components().count() > 1 {
        let path = dir.join(named);
        return path.is_file().then_some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn wait_bounded(child: &mut Child, deadline: Instant) -> Ended {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().map_or(Ended::Killed, Ended::Exited),
            Ok(None) if Instant::now() >= deadline => {
                kill_group(child);
                let _reaped = child.wait();
                return Ended::Timeout;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                kill_group(child);
                return Ended::NotStarted(format!("lost the process: {error}"));
            }
        }
    }
}

/// Kill the child's process group, where there are groups: its id is the
/// child's pid, since it was started as the leader of its own group. One
/// function for every platform, so a test of it covers them all.
fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _status = Command::new("kill")
            .args(["-KILL", "--", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _killed = child.kill();
}

/// Reads `output` to its end into `tail`, keeping its last `TAIL_BYTES`;
/// `io::copy` reads again after an interrupted read.
fn read_tail(output: &mut impl Read, tail: &std::sync::Mutex<std::collections::VecDeque<u8>>) {
    let _copied = std::io::copy(output, &mut Tail(tail));
}

/// A writer that keeps the last `TAIL_BYTES` written to it.
struct Tail<'a>(&'a std::sync::Mutex<std::collections::VecDeque<u8>>);

impl std::io::Write for Tail<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut tail = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let excess = tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(TAIL_BYTES);
        tail.drain(..excess);
        tail.extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
