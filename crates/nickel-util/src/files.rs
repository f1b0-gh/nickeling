//! Descriptor-relative traversal: O_NOFOLLOW on every component, including ancestors.
use crate::error::{ensure, Error, Result};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};
pub fn open_child(parent: &File, name: &std::ffi::OsStr, directory: bool) -> Result<File> {
    open_entry_kind(parent, name, Some(directory), false)
}
pub fn open_directory_child_path(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    open_entry_kind(parent, name, Some(true), true)
}
pub fn open_entry(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    open_entry_kind(parent, name, None, false)
}
fn open_entry_kind(
    parent: &File,
    name: &std::ffi::OsStr,
    directory: Option<bool>,
    path_only: bool,
) -> Result<File> {
    ensure!(
        !name.is_empty() && name != "." && name != ".." && !name.as_bytes().contains(&b'/'),
        "A single normal path component is required"
    );
    let name = CString::new(name.as_bytes())?;
    let flags = if path_only {
        libc::O_PATH
    } else {
        libc::O_RDONLY
    } | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory == Some(true) {
            libc::O_DIRECTORY
        } else {
            0
        };
    // SAFETY: parent owns a live fd; name is NUL terminated; openat returns a new fd.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(Error::PathIo {
            path: std::path::PathBuf::from(std::ffi::OsStr::from_bytes(name.as_bytes())),
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: fd >= 0 is newly opened, and ownership is transferred exactly once.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    ensure!(
        match directory {
            Some(true) => metadata.is_dir(),
            Some(false) => metadata.is_file(),
            None => metadata.is_file() || metadata.is_dir(),
        },
        "File type not allowed"
    );
    Ok(file)
}
pub fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}
pub fn open_directory_path(path: &Path) -> Result<File> {
    let path = absolute(path)?;
    let mut dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => dir = open_entry_kind(&dir, name, Some(true), true)?,
            _ => {
                return Err(Error::InvalidPath {
                    path: path.clone(),
                    reason: "Path is not normalized",
                })
            }
        }
    }
    Ok(dir)
}
pub fn open_directory(path: &Path) -> Result<File> {
    let path = absolute(path)?;
    let dir = open_directory_path(&path)?;
    // SAFETY: dir owns the pinned directory; opening "." only requests read access
    // to the final directory, not to its traversal-only ancestors.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(Error::PathIo {
            path,
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: the fresh descriptor is transferred exactly once.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub fn open_regular(path: &Path) -> Result<File> {
    let path = absolute(path)?;
    let parent = open_directory(path.parent().ok_or_else(|| Error::InvalidPath {
        path: path.clone(),
        reason: "Missing parent",
    })?)?;
    open_child(
        &parent,
        path.file_name().ok_or_else(|| Error::InvalidPath {
            path: path.clone(),
            reason: "Missing name",
        })?,
        false,
    )
    .map_err(|e| e.context(format!("Regular file required: {}", path.display())))
}
pub fn read_file(file: File, limit: usize) -> Result<Vec<u8>> {
    let before = file.metadata()?;
    ensure!(before.len() <= limit as u64, "File is too large");
    let mut bytes = Vec::new();
    let mut limited = file.take((limit as u64).saturating_add(1));
    limited.read_to_end(&mut bytes)?;
    let after = limited.get_ref().metadata()?;
    ensure!(bytes.len() <= limit, "File is too large");
    ensure!(
        before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "File changed while being read"
    );
    Ok(bytes)
}
pub fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>> {
    read_file(open_regular(path)?, limit)
}
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid(message: String) -> Error {
    Error::InvalidInput { message }
}

/// Publish complete JSON through a private temporary file in the pinned parent.
/// Existing regular files are replaced only when explicitly requested.
pub fn publish_json(path: &Path, value: &impl serde::Serialize, replace: bool) -> Result<()> {
    let path = absolute(path)?;
    let parent = open_directory(path.parent().ok_or_else(|| Error::InvalidPath {
        path: path.clone(),
        reason: "Missing parent",
    })?)?;
    let directory = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    let target = directory.join(path.file_name().ok_or_else(|| Error::InvalidPath {
        path: path.clone(),
        reason: "Missing file name",
    })?);
    if replace {
        match std::fs::symlink_metadata(&target) {
            Ok(meta) => ensure!(meta.is_file(), "Destination is not a regular file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(&target).map_err(|e| Error::PathIo {
            path: target.clone(),
            source: e.error,
        })?;
    } else {
        temporary.persist_noclobber(&target).map_err(|e| e.error)?;
    }
    parent.sync_all()?;
    Ok(())
}
