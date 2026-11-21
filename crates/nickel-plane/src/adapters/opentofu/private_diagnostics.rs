use super::diagnostics::{artifact_id, Cause, Phase, Retention, RetentionReason};
use nickel_util::{
    files,
    process::{Captured, Completion},
};
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
const DIRECTORY: &str = "opentofu-diagnostics";
const MAX_FILES: usize = 16;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const STREAM_BYTES: usize = 256 * 1024;
const HEADER_BYTES: usize = 4096;
const MAGIC: &[u8] = b"NICKELING-OPENTOFU-DIAGNOSTICS 1\n";

pub(super) fn retain(
    work: &Path,
    phase: Phase,
    cause: &Cause,
    output: Option<&Captured>,
) -> Retention {
    retain_with(
        work,
        phase,
        cause,
        output,
        |file, header, stdout, stderr| {
            file.write_all(header)?;
            file.write_all(stdout)?;
            file.write_all(stderr)?;
            file.sync_all()
        },
    )
}
fn retain_with(
    work: &Path,
    phase: Phase,
    cause: &Cause,
    output: Option<&Captured>,
    write: impl FnOnce(&mut File, &[u8], &[u8], &[u8]) -> std::io::Result<()>,
) -> Retention {
    retain_with_sync(work, phase, cause, output, write, File::sync_all)
}
fn retain_with_sync(
    work: &Path,
    phase: Phase,
    cause: &Cause,
    output: Option<&Captured>,
    write: impl FnOnce(&mut File, &[u8], &[u8], &[u8]) -> std::io::Result<()>,
    sync: impl FnMut(&File) -> std::io::Result<()>,
) -> Retention {
    match save(work, phase, cause, output, write, sync) {
        Ok(id) => Retention::Saved(id),
        Err(reason) => Retention::Unavailable(reason),
    }
}
fn private(metadata: &std::fs::Metadata, mode: u32) -> bool {
    // SAFETY: geteuid takes no arguments and has no memory effects.
    metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o7777 == mode
}
fn save(
    work: &Path,
    phase: Phase,
    cause: &Cause,
    output: Option<&Captured>,
    write: impl FnOnce(&mut File, &[u8], &[u8], &[u8]) -> std::io::Result<()>,
    mut sync: impl FnMut(&File) -> std::io::Result<()>,
) -> Result<String, RetentionReason> {
    use RetentionReason::{Busy, Io, Quota, UnsafePath};
    let parent = files::open_directory(work).map_err(|_| UnsafePath)?;
    if !private(&parent.metadata().map_err(|_| Io)?, 0o700) {
        return Err(UnsafePath);
    }
    let pinned = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    match std::fs::DirBuilder::new()
        .mode(0o700)
        .create(pinned.join(DIRECTORY))
    {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Io),
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(pinned.join(DIRECTORY))
        .map_err(|_| UnsafePath)?;
    if !private(&directory.metadata().map_err(|_| Io)?, 0o700) {
        return Err(UnsafePath);
    }
    // SAFETY: directory owns this live descriptor; closing it releases flock.
    if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
                Busy
            } else {
                Io
            },
        );
    }
    // Every writer makes the directory name durable, including after a racing creator.
    sync(&parent).map_err(|_| Io)?;
    let pinned = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in std::fs::read_dir(&pinned).map_err(|_| Io)? {
        count += 1;
        if count > MAX_FILES {
            return Err(Quota);
        }
        let entry = entry.map_err(|_| Io)?;
        if !entry.file_name().to_str().is_some_and(artifact_id) {
            return Err(UnsafePath);
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|_| Io)?;
        if !metadata.is_file() || metadata.nlink() != 1 || !private(&metadata, 0o600) {
            return Err(UnsafePath);
        }
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(Quota);
        }
        total = total.checked_add(metadata.len()).ok_or(Quota)?;
        if total > MAX_TOTAL_BYTES {
            return Err(Quota);
        }
    }
    let stdout = output.map_or(&[][..], |o| o.stdout.as_slice());
    let stderr = output.map_or(&[][..], |o| o.stderr.as_slice());
    let kept_out = if phase.ui() {
        &stdout[stdout.len().saturating_sub(STREAM_BYTES)..]
    } else {
        &[]
    };
    let kept_err = &stderr[stderr.len().saturating_sub(STREAM_BYTES)..];
    let metadata = json!({"version":1,"phase":phase.name(),"cause":cause.to_string(),
        "stdoutCapturedBytes":stdout.len(),"stdoutRetainedBytes":kept_out.len(),"stdoutOmittedCapturedBytes":stdout.len()-kept_out.len(),
        "stderrCapturedBytes":stderr.len(),"stderrRetainedBytes":kept_err.len(),"stderrOmittedCapturedBytes":stderr.len()-kept_err.len(),
        "stdoutSelection":if phase.ui(){"suffix"}else{"omitted"},"stderrSelection":"suffix",
        "captureIncomplete":output.is_some_and(|o|matches!(o.completion,Completion::TimedOut|Completion::OutputLimit{..}))});
    let mut header = MAGIC.to_vec();
    serde_json::to_writer(&mut header, &metadata).map_err(|_| Io)?;
    header.push(b'\n');
    if header.len() > HEADER_BYTES {
        return Err(Quota);
    }
    let size = header
        .len()
        .checked_add(kept_out.len())
        .and_then(|s| s.checked_add(kept_err.len()))
        .ok_or(Quota)?;
    if count >= MAX_FILES
        || size > MAX_FILE_BYTES
        || total
            .checked_add(size as u64)
            .is_none_or(|s| s > MAX_TOTAL_BYTES)
    {
        return Err(Quota);
    }
    let temporary = tempfile::Builder::new()
        .prefix("failure-")
        .rand_bytes(12)
        .suffix(".log")
        .tempfile_in(&pinned)
        .map_err(|_| Io)?;
    // Keep before writing: partial diagnostics are evidence, counted on restart.
    let (mut file, path) = temporary.keep().map_err(|_| Io)?;
    let metadata = file.metadata().map_err(|_| Io)?;
    if !metadata.is_file() || metadata.nlink() != 1 || !private(&metadata, 0o600) {
        return Err(UnsafePath);
    }
    let id = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| artifact_id(s))
        .ok_or(UnsafePath)?
        .to_owned();
    sync(&file).map_err(|_| Io)?;
    sync(&directory).map_err(|_| Io)?;
    write(&mut file, &header, kept_out, kept_err).map_err(|_| Io)?;
    sync(&directory).map_err(|_| Io)?;
    Ok(id)
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        os::unix::{fs::symlink, fs::PermissionsExt},
    };
    fn workspace() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        d
    }
    fn sample() -> Captured {
        Captured {
            completion: Completion::TimedOut,
            stdout: vec![b'o'; 16 * 1024 * 1024],
            stderr: vec![b'e'; 16 * 1024 * 1024],
        }
    }
    fn saved(value: Retention) -> String {
        match value {
            Retention::Saved(id) => Some(id),
            _ => None,
        }
        .unwrap()
    }
    #[test]
    fn binary_framing_preserves_bytes_and_omits_data_stdout() {
        let d = workspace();
        let output = Captured {
            completion: Completion::TimedOut,
            stdout: vec![0, 255, 1],
            stderr: vec![254, 0, 2],
        };
        for phase in [Phase::Plan, Phase::Version, Phase::Show] {
            let id = saved(retain(d.path(), phase, &Cause::Timeout(1), Some(&output)));
            let bytes = std::fs::read(d.path().join(DIRECTORY).join(id)).unwrap();
            assert!(bytes.starts_with(MAGIC));
            let end = bytes[MAGIC.len()..]
                .iter()
                .position(|b| *b == b'\n')
                .unwrap()
                + MAGIC.len()
                + 1;
            assert_eq!(
                &bytes[end..],
                if phase.ui() {
                    &[0, 255, 1, 254, 0, 2][..]
                } else {
                    &[254, 0, 2][..]
                }
            );
        }
    }
    #[test]
    fn raw_artifacts_have_bounded_framing_permissions_and_private_data() {
        let d = workspace();
        let output = sample();
        let id = saved(retain(
            d.path(),
            Phase::Plan,
            &Cause::Timeout(1),
            Some(&output),
        ));
        assert!(artifact_id(&id));
        let path = d.path().join(DIRECTORY).join(id);
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() <= MAX_FILE_BYTES);
        let body = &bytes[MAGIC.len()..];
        let newline = body.iter().position(|b| *b == b'\n').unwrap();
        let header: serde_json::Value = serde_json::from_slice(&body[..newline]).unwrap();
        assert_eq!(header["stdoutRetainedBytes"], 256 * 1024);
        assert_eq!(header["stderrRetainedBytes"], 256 * 1024);
        assert_eq!(
            header["stdoutOmittedCapturedBytes"],
            16 * 1024 * 1024 - 256 * 1024
        );
        assert_eq!(header["captureIncomplete"], true);
        assert_eq!(body.len() - newline - 1, 512 * 1024);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(d.path().join(DIRECTORY))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let id = saved(retain(
            d.path(),
            Phase::Show,
            &Cause::InvalidJson,
            Some(&output),
        ));
        let bytes = std::fs::read(d.path().join(DIRECTORY).join(id)).unwrap();
        assert!(!bytes.windows(32).any(|v| v == [b'o'; 32]));
    }
    #[test]
    fn stdout_tail_and_repeated_parent_sync_are_required() {
        let d = workspace();
        let mut output = sample();
        output.stdout = vec![b'h'; STREAM_BYTES];
        output.stdout.extend(vec![b't'; STREAM_BYTES]);
        let id = saved(retain(
            d.path(),
            Phase::Plan,
            &Cause::Exit(1),
            Some(&output),
        ));
        let bytes = std::fs::read(d.path().join(DIRECTORY).join(id)).unwrap();
        let header_end = bytes[MAGIC.len()..]
            .iter()
            .position(|b| *b == b'\n')
            .unwrap()
            + MAGIC.len()
            + 1;
        assert!(bytes[header_end..header_end + STREAM_BYTES]
            .iter()
            .all(|b| *b == b't'));
        let before = std::fs::read_dir(d.path().join(DIRECTORY)).unwrap().count();
        let parent_ino = std::fs::metadata(d.path()).unwrap().ino();
        let result = retain_with_sync(
            d.path(),
            Phase::Plan,
            &Cause::Exit(1),
            None,
            |_, _, _, _| Ok(()),
            |file| {
                if file.metadata()?.ino() == parent_ino {
                    Err(std::io::Error::other("private-sync-failure"))
                } else {
                    file.sync_all()
                }
            },
        );
        assert!(matches!(
            result,
            Retention::Unavailable(RetentionReason::Io)
        ));
        assert_eq!(
            std::fs::read_dir(d.path().join(DIRECTORY)).unwrap().count(),
            before
        );
    }
    #[test]
    fn count_and_byte_quotas_refuse_without_deleting_any_evidence() {
        for (count, size) in [(16, 0), (8, 1024 * 1024), (1, 1024 * 1024 + 1)] {
            let d = workspace();
            let directory = d.path().join(DIRECTORY);
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            for index in 0..count {
                let path = directory.join(format!("failure-{index:012}.log"));
                let f = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(path)
                    .unwrap();
                f.set_len(size).unwrap();
            }
            assert!(matches!(
                retain(d.path(), Phase::Init, &Cause::Exit(1), None),
                Retention::Unavailable(RetentionReason::Quota)
            ));
            assert_eq!(
                std::fs::read_dir(directory).unwrap().count(),
                count as usize
            );
        }
    }
    #[test]
    fn prospective_bytes_and_final_sync_failure_preserve_evidence() {
        let d = workspace();
        let directory = d.path().join(DIRECTORY);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        for index in 0..8 {
            let f = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(directory.join(format!("failure-{index:012}.log")))
                .unwrap();
            f.set_len(1024 * 1024 - u64::from(index == 7)).unwrap();
        }
        assert!(matches!(
            retain(d.path(), Phase::Init, &Cause::Exit(1), None),
            Retention::Unavailable(RetentionReason::Quota)
        ));
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 8);
        let d = workspace();
        let mut directory_syncs = 0;
        let result = retain_with_sync(
            d.path(),
            Phase::Init,
            &Cause::Exit(1),
            None,
            |file, header, out, err| {
                file.write_all(header)?;
                file.write_all(out)?;
                file.write_all(err)?;
                file.sync_all()
            },
            |file| {
                if file.metadata()?.is_dir() {
                    directory_syncs += 1;
                    if directory_syncs == 3 {
                        return Err(std::io::Error::other("synthetic-private-sync-failure"));
                    }
                }
                file.sync_all()
            },
        );
        assert!(matches!(
            result,
            Retention::Unavailable(RetentionReason::Io)
        ));
        let entry = std::fs::read_dir(d.path().join(DIRECTORY))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert!(entry.metadata().unwrap().len() > MAGIC.len() as u64);
    }
    #[test]
    fn unsafe_directory_unknown_name_and_fifo_are_refused() {
        for variant in ["directory-link", "directory-mode", "unknown-name", "fifo"] {
            let d = workspace();
            let outside = workspace();
            let directory = d.path().join(DIRECTORY);
            if variant == "directory-link" {
                symlink(outside.path(), &directory).unwrap();
            } else {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&directory)
                    .unwrap();
                match variant {
                    "directory-mode" => {
                        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
                            .unwrap()
                    }
                    "unknown-name" => {
                        std::fs::write(directory.join("user-evidence"), b"preserve").unwrap()
                    }
                    _ => {
                        let name = std::ffi::CString::new(
                            directory
                                .join("failure-000000000001.log")
                                .as_os_str()
                                .as_encoded_bytes(),
                        )
                        .unwrap();
                        // SAFETY: name is a valid NUL-terminated path in the owned test directory.
                        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                    }
                }
            }
            assert!(matches!(
                retain(d.path(), Phase::Init, &Cause::Exit(1), None),
                Retention::Unavailable(RetentionReason::UnsafePath)
            ));
            assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        }
    }
    #[test]
    fn unsafe_entries_and_busy_directory_are_refused() {
        for variant in ["symlink", "hardlink", "mode", "directory"] {
            let d = workspace();
            let directory = d.path().join(DIRECTORY);
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            let outside = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(outside.path(), b"preserve").unwrap();
            let path = directory.join("failure-000000000001.log");
            match variant {
                "symlink" => symlink(outside.path(), &path).unwrap(),
                "hardlink" => std::fs::hard_link(outside.path(), &path).unwrap(),
                "mode" => {
                    std::fs::write(&path, b"preserve").unwrap();
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
                }
                _ => std::fs::create_dir(path).unwrap(),
            }
            assert!(matches!(
                retain(d.path(), Phase::Plan, &Cause::Exit(1), None),
                Retention::Unavailable(RetentionReason::UnsafePath)
            ));
            assert_eq!(std::fs::read(outside.path()).unwrap(), b"preserve");
        }
        let d = workspace();
        let id = saved(retain(d.path(), Phase::Init, &Cause::Exit(1), None));
        assert!(!id.is_empty());
        let directory = File::open(d.path().join(DIRECTORY)).unwrap();
        assert_eq!(
            unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert!(matches!(
            retain(d.path(), Phase::Init, &Cause::Exit(1), None),
            Retention::Unavailable(RetentionReason::Busy)
        ));
    }
    #[test]
    fn partial_write_failure_keeps_counted_evidence_without_success_reference() {
        let d = workspace();
        let directory = d.path().join(DIRECTORY);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        for index in 0..15 {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(directory.join(format!("failure-{index:012}.log")))
                .unwrap();
        }
        let result = retain_with(
            d.path(),
            Phase::Plan,
            &Cause::Exit(1),
            None,
            |file, header, _, _| {
                file.write_all(header)?;
                Err(std::io::Error::other("synthetic-private-write-error"))
            },
        );
        assert!(matches!(
            result,
            Retention::Unavailable(RetentionReason::Io)
        ));
        let paths: Vec<_> = std::fs::read_dir(d.path().join(DIRECTORY))
            .unwrap()
            .collect();
        assert_eq!(paths.len(), 16);
        let partial = paths
            .iter()
            .find(|entry| entry.as_ref().unwrap().metadata().unwrap().len() > 0)
            .unwrap();
        let mut bytes = Vec::new();
        File::open(partial.as_ref().unwrap().path())
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(bytes.starts_with(MAGIC));
        assert!(matches!(
            retain(d.path(), Phase::Plan, &Cause::Exit(1), None),
            Retention::Unavailable(RetentionReason::Quota)
        ));
    }
}
