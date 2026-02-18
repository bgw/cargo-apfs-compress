// Derived from Cargo's `src/cargo/util/flock.rs` implementation.
//
// Copyright Cargo contributors.
//
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Note: This file is licensed as "MIT OR Apache-2.0" (Cargo's license),
// not GPL-3.0-or-later.

//! File-locking support.

use std::fs::TryLockError;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Display, Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::progress::ProgressBars;

#[derive(Debug)]
pub struct FileLock {
    f: Option<File>,
    path: PathBuf,
}

impl FileLock {
    pub fn file(&self) -> &File {
        self.f.as_ref().expect("file lock missing file handle")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn parent(&self) -> &Path {
        self.path.parent().expect("lock has no parent")
    }

    pub fn remove_siblings(&self) -> Result<()> {
        let path = self.path();
        for entry in path.parent().expect("lock has no parent").read_dir()? {
            let entry = entry?;
            if Some(&entry.file_name()[..]) == path.file_name() {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    pub fn rename<P: AsRef<Path>>(&mut self, new_path: P) -> Result<()> {
        let new_path = new_path.as_ref();
        std::fs::rename(&self.path, new_path).with_context(|| {
            format!(
                "failed to rename {} to {}",
                self.path.display(),
                new_path.display()
            )
        })?;
        self.path = new_path.to_path_buf();
        Ok(())
    }
}

impl Read for FileLock {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file().read(buf)
    }
}

impl Seek for FileLock {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.file().seek(to)
    }
}

impl Write for FileLock {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file().flush()
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Some(f) = self.f.take() {
            let _ = f.unlock();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filesystem {
    root: PathBuf,
}

impl Filesystem {
    pub fn new(path: PathBuf) -> Filesystem {
        Filesystem { root: path }
    }

    pub fn join<T: AsRef<Path>>(&self, other: T) -> Filesystem {
        Filesystem::new(self.root.join(other))
    }

    pub fn push<T: AsRef<Path>>(&mut self, other: T) {
        self.root.push(other);
    }

    pub fn into_path_unlocked(self) -> PathBuf {
        self.root
    }

    pub fn as_path_unlocked(&self) -> &Path {
        &self.root
    }

    pub fn create_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        Ok(())
    }

    pub fn display(&self) -> Display<'_> {
        self.root.display()
    }

    pub fn open_rw_exclusive_create<P>(
        &self,
        path: P,
        msg: &str,
        progress: &ProgressBars,
    ) -> Result<FileLock>
    where
        P: AsRef<Path>,
    {
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        let (path, f) = self.open(path.as_ref(), &opts, true)?;
        acquire(msg, &path, progress, &|| f.try_lock(), &|| f.lock())?;
        Ok(FileLock { f: Some(f), path })
    }

    fn open(&self, path: &Path, opts: &OpenOptions, create: bool) -> Result<(PathBuf, File)> {
        let path = self.root.join(path);
        let f = opts
            .open(&path)
            .or_else(|e| {
                if e.kind() == io::ErrorKind::NotFound && create {
                    std::fs::create_dir_all(path.parent().expect("lock file has no parent"))?;
                    Ok(opts.open(&path)?)
                } else {
                    Err(anyhow::Error::from(e))
                }
            })
            .with_context(|| format!("failed to open: {}", path.display()))?;
        Ok((path, f))
    }
}

impl PartialEq<Path> for Filesystem {
    fn eq(&self, other: &Path) -> bool {
        self.root == other
    }
}

impl PartialEq<Filesystem> for Path {
    fn eq(&self, other: &Filesystem) -> bool {
        self == other.root
    }
}

fn try_acquire(path: &Path, lock_try: &dyn Fn() -> Result<(), TryLockError>) -> Result<bool> {
    if is_on_nfs_mount(path) {
        return Ok(true);
    }

    match lock_try() {
        Ok(()) => Ok(true),
        Err(TryLockError::Error(e)) if error_unsupported(&e) => Ok(true),
        Err(TryLockError::Error(e)) => {
            let e = anyhow::Error::from(e);
            let cx = format!("failed to lock file: {}", path.display());
            Err(e.context(cx))
        }
        Err(TryLockError::WouldBlock) => Ok(false),
    }
}

fn acquire(
    msg: &str,
    path: &Path,
    progress: &ProgressBars,
    lock_try: &dyn Fn() -> Result<(), TryLockError>,
    lock_block: &dyn Fn() -> io::Result<()>,
) -> Result<()> {
    if try_acquire(path, lock_try)? {
        return Ok(());
    }

    progress.println_normal(|| format!("Blocking waiting for file lock on {msg}"));
    lock_block().with_context(|| format!("failed to lock file: {}", path.display()))?;
    Ok(())
}

fn is_on_nfs_mount(_path: &Path) -> bool {
    false
}

fn error_unsupported(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Unsupported
}
