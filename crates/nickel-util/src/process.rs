//! Bounded subprocess capture, including descendants retaining inherited pipes.
use crate::error::{Error, Result};
use std::{
    io::{ErrorKind, Read, Seek, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
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
pub fn capture(command: Command, timeout: Duration, limit: usize) -> Result<Vec<u8>> {
    capture_inner(command, timeout, limit, Stdio::null())
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
    capture_inner(command, timeout, limit, Stdio::from(file))
}
fn capture_inner(
    mut command: Command,
    timeout: Duration,
    limit: usize,
    input: Stdio,
) -> Result<Vec<u8>> {
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
    loop {
        drain(&mut out, &mut stdout, limit, &program, "stdout")?;
        drain(&mut err, &mut stderr, limit, &program, "stderr")?;
        if let Some(status) = child.0.try_wait()? {
            drain(&mut out, &mut stdout, limit, &program, "stdout")?;
            drain(&mut err, &mut stderr, limit, &program, "stderr")?;
            if !status.success() {
                return Err(Error::ProcessExit {
                    program: program.clone(),
                    status,
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }
            return Ok(stdout);
        }
        if start.elapsed() >= timeout {
            return Err(Error::ProcessTimeout {
                program: program.clone(),
                timeout,
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}
