//! Host-private directory exclusion using retained filesystem identities.

use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use rustix::fs::{self, Mode, OFlags};

use crate::SafePathError;

/// A retained host-private directory, never supplied through model or frontend input.
#[derive(Clone, Debug)]
pub struct PrivateDirectory {
    directory: Arc<File>,
    path: PathBuf,
}

impl PrivateDirectory {
    /// Retains the private directory so renames cannot redirect its exclusion.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be opened without following a symlink.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SafePathError> {
        let fd = fs::open(
            path.as_ref(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        Ok(Self {
            directory: Arc::new(File::from(fd)),
            path: path.as_ref().canonicalize().map_err(SafePathError::Io)?,
        })
    }

    pub(crate) fn reject_overlap(&self, directory: &File) -> Result<(), SafePathError> {
        reject_overlap(&self.directory, directory)?;
        // Retain the original identity after a move, but also reserve the
        // configured location if another directory is created in its place.
        match fs::open(&self.path, ancestor_flags(), Mode::empty()) {
            Ok(fd) => reject_overlap(&File::from(fd), directory),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }
}

fn reject_overlap(private: &File, directory: &File) -> Result<(), SafePathError> {
    if ancestor_of(private, directory)? || ancestor_of(directory, private)? {
        return Err(SafePathError::ProtectedPath);
    }
    Ok(())
}

fn ancestor_of(ancestor: &File, child: &File) -> Result<bool, SafePathError> {
    let target = fs::fstat(ancestor).map_err(io_error)?;
    let mut current = child.try_clone().map_err(SafePathError::Io)?;
    // Bound even corrupt/unusual directory ancestry; uncertainty denies access.
    for _ in 0..256 {
        let stat = fs::fstat(&current).map_err(io_error)?;
        if stat.st_dev == target.st_dev && stat.st_ino == target.st_ino {
            return Ok(true);
        }
        let parent = fs::openat(&current, "..", ancestor_flags(), Mode::empty())
            .map(File::from)
            .map_err(io_error)?;
        let parent_stat = fs::fstat(&parent).map_err(io_error)?;
        if stat.st_dev == parent_stat.st_dev && stat.st_ino == parent_stat.st_ino {
            return Ok(false);
        }
        current = parent;
    }
    Err(SafePathError::ProtectedPath)
}

fn ancestor_flags() -> OFlags {
    // Sandbox access to the workspace/private directory does not grant listing
    // access to /Users or /. Request traversal only, as the vault storage does.
    #[cfg(target_os = "macos")]
    let flags = OFlags::from_bits_retain(libc::O_SEARCH as u32);
    #[cfg(not(target_os = "macos"))]
    let flags = OFlags::RDONLY | OFlags::DIRECTORY;
    flags | OFlags::CLOEXEC | OFlags::NOFOLLOW
}

fn io_error(error: rustix::io::Errno) -> SafePathError {
    SafePathError::Io(error.into())
}

#[cfg(test)]
mod tests;
