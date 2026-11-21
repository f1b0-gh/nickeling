//! Bounded subprocess capture, including descendants retaining inherited pipes.
use crate::error::{Error, Result};
use std::{
    io::{ErrorKind, Read, Seek, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // SAFETY: kill only targets the process group created for this child.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}
fn nonblocking(stream: &impl AsRawFd) -> Result<()> {
    let fd = stream.as_raw_fd();
    // SAFETY: the borrowed stream keeps fd alive for both fcntl calls.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn drain(
    stream: &mut impl Read,
    bytes: &mut Vec<u8>,
    limit: usize,
    program: &std::ffi::OsStr,
    name: &'static str,
) -> Result<()> {
    let mut buffer = [0; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if n > limit.saturating_sub(bytes.len()) {
                    return Err(Error::ProcessOutputLimit {
                        program: program.into(),
                        stream: name,
                        limit,
                    });
                }
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Debug)]
pub enum Completion {
    Exited(ExitStatus),
    TimedOut,
    OutputLimit { stream: Stream },
}
pub struct Captured {
    pub completion: Completion,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl std::fmt::Debug for Captured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Captured")
            .field("completion", &self.completion)
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr_bytes", &self.stderr.len())
            .finish()
    }
}
/// Both streams remain private data, including on failure. Debug shows only lengths.
pub fn capture_outcome(command: Command, timeout: Duration, limit: usize) -> Result<Captured> {
    capture_inner(command, timeout, limit, Stdio::null())
}
pub fn capture(command: Command, timeout: Duration, limit: usize) -> Result<Vec<u8>> {
    legacy_capture(command, timeout, limit, Stdio::null())
}
fn legacy_capture(
    command: Command,
    timeout: Duration,
    limit: usize,
    input: Stdio,
) -> Result<Vec<u8>> {
    let program = command.get_program().to_os_string();
    let output = capture_inner(command, timeout, limit, input)?;
    match output.completion {
        Completion::Exited(status) if status.success() => Ok(output.stdout),
        Completion::Exited(status) => Err(Error::ProcessExit {
            program,
            status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Completion::TimedOut => Err(Error::ProcessTimeout { program, timeout }),
        Completion::OutputLimit { stream } => Err(Error::ProcessOutputLimit {
            program,
            stream: match stream {
                Stream::Stdout => "stdout",
                Stream::Stderr => "stderr",
            },
            limit,
        }),
    }
}
/// A private seekable input avoids deadlock when a bounded child never reads stdin.
pub fn capture_input(
    command: Command,
    input: &[u8],
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>> {
    if input.len() > 16 * 1024 * 1024 {
        return Err(Error::InvalidInput {
            message: "Process input exceeds 16 MiB".into(),
        });
    }
    let mut file = tempfile::tempfile()?;
    file.write_all(input)?;
    file.rewind()?;
    legacy_capture(command, timeout, limit, Stdio::from(file))
}
fn capture_inner(
    mut command: Command,
    timeout: Duration,
    limit: usize,
    input: Stdio,
) -> Result<Captured> {
    command
        .stdin(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let program = command.get_program().to_os_string();
    let mut child = ChildGuard(command.spawn().map_err(|source| Error::ProcessSpawn {
        program: program.clone(),
        source,
    })?);
    let mut out = child.0.stdout.take().ok_or_else(|| Error::MissingPipe {
        program: program.clone(),
        stream: "stdout",
    })?;
    let mut err = child.0.stderr.take().ok_or_else(|| Error::MissingPipe {
        program: program.clone(),
        stream: "stderr",
    })?;
    nonblocking(&out)?;
    nonblocking(&err)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let start = Instant::now();
    let completion = loop {
        let streams = drain(&mut out, &mut stdout, limit, &program, "stdout")
            .and_then(|()| drain(&mut err, &mut stderr, limit, &program, "stderr"));
        if let Err(error) = streams {
            match error {
                Error::ProcessOutputLimit { stream, .. } => {
                    break Completion::OutputLimit {
                        stream: if stream == "stdout" {
                            Stream::Stdout
                        } else {
                            Stream::Stderr
                        },
                    }
                }
                other => return Err(other),
            }
        }
        if let Some(status) = child.0.try_wait()? {
            // Drain once more after exit; descendants cannot keep us waiting.
            let streams = drain(&mut out, &mut stdout, limit, &program, "stdout")
                .and_then(|()| drain(&mut err, &mut stderr, limit, &program, "stderr"));
            if let Err(error) = streams {
                match error {
                    Error::ProcessOutputLimit { stream, .. } => {
                        break Completion::OutputLimit {
                            stream: if stream == "stdout" {
                                Stream::Stdout
                            } else {
                                Stream::Stderr
                            },
                        }
                    }
                    other => return Err(other),
                }
            }
            break Completion::Exited(status);
        }
        if start.elapsed() >= timeout {
            break Completion::TimedOut;
        }
        thread::sleep(Duration::from_millis(10));
    };
    drop(child);
    Ok(Captured {
        completion,
        stdout,
        stderr,
    })
}
