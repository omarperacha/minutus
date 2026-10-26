//! Capability-based workspace path traversal.

use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt,
    fs::File,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use rustix::{
    fs::{self, AtFlags, Dir, FileType, Mode, OFlags},
    io::Errno,
};

/// Stable identity of an opened filesystem object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceIdentity {
    device: u64,
    inode: u64,
}

impl WorkspaceIdentity {
    /// Returns the device containing the object.
    #[must_use]
    pub const fn device(self) -> u64 {
        self.device
    }

    /// Returns the inode of the object on its device.
    #[must_use]
    pub const fn inode(self) -> u64 {
        self.inode
    }
}

/// An opened directory capability anchoring all workspace-relative access.
#[derive(Debug)]
pub struct WorkspaceDirectory {
    root: File,
    identity: WorkspaceIdentity,
    private_directory: Option<crate::PrivateDirectory>,
}

impl WorkspaceDirectory {
    /// Opens a trusted workspace root without following a symbolic link.
    ///
    /// The resulting file descriptor, rather than `root`, is the authority used
    /// by every later operation. Renaming the root therefore does not redirect
    /// access to a replacement created at the old path.
    ///
    /// # Errors
    ///
    /// Returns an error when the root is missing, protected by host policy, is
    /// a symlink or is not a stable directory capability.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, SafePathError> {
        // This name check only applies exclusion policy. The opened descriptor
        // remains the authority for every later filesystem operation.
        crate::path_policy::validate_path(
            &root.as_ref().canonicalize().map_err(SafePathError::Io)?,
        )?;
        let root = fs::open(
            root.as_ref(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(open_error)?;
        let root = File::from(root);
        let stat = fs::fstat(&root).map_err(io_error)?;
        if !FileType::from_raw_mode(stat.st_mode).is_dir() {
            return Err(SafePathError::UnexpectedFileType);
        }
        let identity = identity(&stat);
        Ok(Self {
            root,
            identity,
            private_directory: None,
        })
    }

    /// Opens a workspace while excluding the host's private data and its ancestors.
    ///
    /// # Errors
    /// Returns an error if the root overlaps private data or cannot be checked.
    pub fn open_excluding(
        root: impl AsRef<Path>,
        private: &crate::PrivateDirectory,
    ) -> Result<Self, SafePathError> {
        let mut workspace = Self::open(root)?;
        private.reject_overlap(&workspace.root)?;
        workspace.private_directory = Some(private.clone());
        Ok(workspace)
    }

    fn validate_opened(&self, opened: &File, expected: ExpectedType) -> Result<(), SafePathError> {
        validate_opened(opened, self.identity.device, expected)?;
        if expected == ExpectedType::Directory
            && let Some(private) = &self.private_directory
        {
            private.reject_overlap(opened)?;
        }
        Ok(())
    }

    /// Returns the identity captured from the opened workspace root.
    #[must_use]
    pub const fn identity(&self) -> WorkspaceIdentity {
        self.identity
    }

    /// Opens one regular file beneath the workspace with handle-relative,
    /// no-follow traversal.
    ///
    /// Every component must exist with the exact requested byte spelling.
    /// Symlinks, Finder aliases, special files and mounted sub-volumes are
    /// rejected. The final type and device are checked on the opened handle.
    ///
    /// # Errors
    ///
    /// Returns an error when the relative path is malformed, any component is
    /// unsafe, the path crosses a mount, or the file cannot be opened safely.
    pub fn open_file(&self, relative_path: impl AsRef<Path>) -> Result<File, SafePathError> {
        self.open_entry(relative_path.as_ref(), ExpectedType::RegularFile, |_, _| {})
    }

    /// Opens one directory beneath the workspace with handle-relative,
    /// no-follow traversal.
    ///
    /// # Errors
    ///
    /// Returns an error when the relative path is malformed, any component is
    /// unsafe, the path crosses a mount, or the directory cannot be opened safely.
    pub fn open_directory(&self, relative_path: impl AsRef<Path>) -> Result<File, SafePathError> {
        self.open_entry(relative_path.as_ref(), ExpectedType::Directory, |_, _| {})
    }

    pub(crate) fn resolve_write_target(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<WriteTarget, SafePathError> {
        let components = validate_relative_path(relative_path.as_ref())?;
        let (name, parents) = components
            .split_last()
            .ok_or(SafePathError::InvalidRelativePath)?;
        let mut parent = self.root.try_clone().map_err(SafePathError::Io)?;
        self.validate_opened(&parent, ExpectedType::Directory)?;
        for component in parents {
            require_exact_entry(&parent, component)?;
            reject_link_or_wrong_type(&parent, component, ExpectedType::Directory)?;
            let opened = open_component(&parent, component, ExpectedType::Directory)?;
            self.validate_opened(&opened, ExpectedType::Directory)?;
            parent = opened;
        }
        let existing = match fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => {
                require_exact_entry(&parent, name)?;
                reject_link_or_wrong_type(&parent, name, ExpectedType::RegularFile)?;
                let opened = open_component(&parent, name, ExpectedType::RegularFile)?;
                self.validate_opened(&opened, ExpectedType::RegularFile)?;
                reject_finder_alias(&opened)?;
                Some(opened)
            }
            Err(Errno::NOENT) => None,
            Err(error) => return Err(open_error(error)),
        };
        Ok(WriteTarget {
            parent,
            name: name.clone(),
            existing,
        })
    }

    pub(crate) fn resolve_new_directory_target(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<DirectoryTarget, SafePathError> {
        let components = validate_relative_path(relative_path.as_ref())?;
        let (name, parents) = components
            .split_last()
            .ok_or(SafePathError::InvalidRelativePath)?;
        let mut parent = self.root.try_clone().map_err(SafePathError::Io)?;
        self.validate_opened(&parent, ExpectedType::Directory)?;
        for component in parents {
            require_exact_entry(&parent, component)?;
            reject_link_or_wrong_type(&parent, component, ExpectedType::Directory)?;
            let opened = open_component(&parent, component, ExpectedType::Directory)?;
            self.validate_opened(&opened, ExpectedType::Directory)?;
            parent = opened;
        }
        match fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(Errno::NOENT) => Ok(DirectoryTarget {
                parent,
                name: name.clone(),
            }),
            Ok(_) => Err(SafePathError::UnexpectedFileType),
            Err(error) => Err(open_error(error)),
        }
    }

    pub(crate) fn create_missing_parent_directories_for(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<Vec<String>, SafePathError> {
        let components = validate_relative_path(relative_path.as_ref())?;
        let (_, parents) = components
            .split_last()
            .ok_or(SafePathError::InvalidRelativePath)?;
        let mut parent = self.root.try_clone().map_err(SafePathError::Io)?;
        self.validate_opened(&parent, ExpectedType::Directory)?;
        let mut traversed = PathBuf::new();
        let mut created = Vec::new();
        let result = (|| {
            for component in parents {
                traversed.push(component);
                match fs::statat(&parent, component, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(_) => {
                        require_exact_entry(&parent, component)?;
                        reject_link_or_wrong_type(&parent, component, ExpectedType::Directory)?;
                    }
                    Err(Errno::NOENT) => {
                        fs::mkdirat(&parent, component, Mode::from_bits_truncate(0o755))
                            .map_err(io_error)?;
                        fs::fsync(&parent).map_err(io_error)?;
                        created.push(traversed.to_string_lossy().into_owned());
                    }
                    Err(error) => return Err(open_error(error)),
                }
                let opened = open_component(&parent, component, ExpectedType::Directory)?;
                self.validate_opened(&opened, ExpectedType::Directory)?;
                parent = opened;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.remove_empty_directories(&created);
            return Err(error);
        }
        Ok(created)
    }

    pub(crate) fn remove_empty_directories(&self, relative_paths: &[String]) {
        for relative_path in relative_paths.iter().rev() {
            let Ok(components) = validate_relative_path(Path::new(relative_path)) else {
                continue;
            };
            let Some((name, parents)) = components.split_last() else {
                continue;
            };
            let Ok(mut parent) = self.root.try_clone() else {
                continue;
            };
            let mut valid = true;
            for component in parents {
                if require_exact_entry(&parent, component).is_err()
                    || reject_link_or_wrong_type(&parent, component, ExpectedType::Directory)
                        .is_err()
                {
                    valid = false;
                    break;
                }
                let Ok(opened) = open_component(&parent, component, ExpectedType::Directory) else {
                    valid = false;
                    break;
                };
                if self
                    .validate_opened(&opened, ExpectedType::Directory)
                    .is_err()
                {
                    valid = false;
                    break;
                }
                parent = opened;
            }
            if valid && fs::unlinkat(&parent, name, AtFlags::REMOVEDIR).is_ok() {
                let _ = fs::fsync(&parent);
            }
        }
    }

    fn open_entry<F>(
        &self,
        relative_path: &Path,
        expected_type: ExpectedType,
        mut before_open: F,
    ) -> Result<File, SafePathError>
    where
        F: FnMut(usize, &OsStr),
    {
        if relative_path == Path::new(".") {
            if expected_type != ExpectedType::Directory {
                return Err(SafePathError::UnexpectedFileType);
            }
            let root = self.root.try_clone().map_err(SafePathError::Io)?;
            self.validate_opened(&root, ExpectedType::Directory)?;
            return Ok(root);
        }
        let components = validate_relative_path(relative_path)?;
        let mut parent = self.root.try_clone().map_err(SafePathError::Io)?;
        self.validate_opened(&parent, ExpectedType::Directory)?;
        let last_index = components.len().saturating_sub(1);

        for (index, component) in components.iter().enumerate() {
            require_exact_entry(&parent, component)?;
            let expected = if index == last_index {
                expected_type
            } else {
                ExpectedType::Directory
            };
            reject_link_or_wrong_type(&parent, component, expected)?;
            before_open(index, component);
            let opened = open_component(&parent, component, expected)?;
            self.validate_opened(&opened, expected)?;
            if expected == ExpectedType::RegularFile {
                reject_finder_alias(&opened)?;
            }
            parent = opened;
        }

        Ok(parent)
    }
}

pub(crate) struct WriteTarget {
    pub(crate) parent: File,
    pub(crate) name: OsString,
    pub(crate) existing: Option<File>,
}

pub(crate) struct DirectoryTarget {
    pub(crate) parent: File,
    pub(crate) name: OsString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedType {
    Directory,
    RegularFile,
}

/// Typed failures from workspace-relative path resolution.
#[derive(Debug)]
pub enum SafePathError {
    /// The supplied path was empty, absolute or contained a non-normal component.
    InvalidRelativePath,
    /// No directory entry has the exact requested byte spelling.
    NotFound,
    /// A symbolic link was encountered.
    SymbolicLink,
    /// A macOS Finder alias was encountered.
    FinderAlias,
    /// The opened object was not the requested regular-file or directory type.
    UnexpectedFileType,
    /// Traversal attempted to enter a different mounted filesystem.
    CrossDevice,
    /// The path names credentials or protected execution-control configuration.
    ProtectedPath,
    /// A platform filesystem operation failed closed.
    Io(std::io::Error),
}

impl fmt::Display for SafePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRelativePath => {
                formatter.write_str("workspace path must be a strict relative path")
            }
            Self::NotFound => formatter.write_str("workspace entry was not found exactly"),
            Self::SymbolicLink => formatter.write_str("workspace path contains a symbolic link"),
            Self::FinderAlias => formatter.write_str("workspace path is a Finder alias"),
            Self::UnexpectedFileType => {
                formatter.write_str("workspace entry has an unsupported file type")
            }
            Self::CrossDevice => formatter.write_str("workspace path crosses a mounted filesystem"),
            Self::ProtectedPath => {
                formatter.write_str("workspace path is protected by host policy")
            }
            Self::Io(error) => write!(formatter, "workspace path access failed: {error}"),
        }
    }
}

impl Error for SafePathError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidRelativePath
            | Self::NotFound
            | Self::SymbolicLink
            | Self::FinderAlias
            | Self::UnexpectedFileType
            | Self::CrossDevice
            | Self::ProtectedPath => None,
        }
    }
}

fn validate_relative_path(path: &Path) -> Result<Vec<OsString>, SafePathError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.starts_with(b"/") || bytes.ends_with(b"/") {
        return Err(SafePathError::InvalidRelativePath);
    }
    let mut components = Vec::new();
    for component in bytes.split(|byte| *byte == b'/') {
        if component.is_empty() || component == b"." || component == b".." || component.contains(&0)
        {
            return Err(SafePathError::InvalidRelativePath);
        }
        components.push(OsStr::from_bytes(component).to_os_string());
    }
    crate::path_policy::validate_path(path)?;
    Ok(components)
}

fn require_exact_entry(parent: &File, component: &OsStr) -> Result<(), SafePathError> {
    let mut directory = Dir::read_from(parent).map_err(io_error)?;
    for entry in &mut directory {
        let entry = entry.map_err(io_error)?;
        if entry.file_name().to_bytes() == component.as_bytes() {
            return Ok(());
        }
    }
    Err(SafePathError::NotFound)
}

fn reject_link_or_wrong_type(
    parent: &File,
    component: &OsStr,
    expected: ExpectedType,
) -> Result<(), SafePathError> {
    let stat = fs::statat(parent, component, AtFlags::SYMLINK_NOFOLLOW).map_err(open_error)?;
    let file_type = FileType::from_raw_mode(stat.st_mode);
    if file_type.is_symlink() {
        return Err(SafePathError::SymbolicLink);
    }
    let valid = match expected {
        ExpectedType::Directory => file_type.is_dir(),
        ExpectedType::RegularFile => file_type.is_file(),
    };
    if !valid {
        return Err(SafePathError::UnexpectedFileType);
    }
    Ok(())
}

fn open_component(
    parent: &File,
    component: &OsStr,
    expected: ExpectedType,
) -> Result<File, SafePathError> {
    let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    if expected == ExpectedType::Directory {
        flags |= OFlags::DIRECTORY;
    }
    fs::openat(parent, component, flags, Mode::empty())
        .map(File::from)
        .map_err(open_error)
}

fn validate_opened(
    opened: &File,
    root_device: u64,
    expected: ExpectedType,
) -> Result<(), SafePathError> {
    let stat = fs::fstat(opened).map_err(io_error)?;
    let opened_identity = identity(&stat);
    if opened_identity.device != root_device {
        return Err(SafePathError::CrossDevice);
    }
    let file_type = FileType::from_raw_mode(stat.st_mode);
    let valid = match expected {
        ExpectedType::Directory => file_type.is_dir(),
        ExpectedType::RegularFile => file_type.is_file(),
    };
    if !valid {
        return Err(SafePathError::UnexpectedFileType);
    }
    Ok(())
}

fn identity(stat: &fs::Stat) -> WorkspaceIdentity {
    WorkspaceIdentity {
        device: device_number(stat),
        inode: inode_number(stat),
    }
}

#[cfg(target_os = "macos")]
fn device_number(stat: &fs::Stat) -> u64 {
    u64::from(u32::from_ne_bytes(stat.st_dev.to_ne_bytes()))
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::cast_sign_loss)]
fn device_number(stat: &fs::Stat) -> u64 {
    stat.st_dev as u64
}

#[cfg(target_os = "macos")]
fn inode_number(stat: &fs::Stat) -> u64 {
    stat.st_ino
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::cast_lossless, clippy::cast_sign_loss)]
fn inode_number(stat: &fs::Stat) -> u64 {
    stat.st_ino as u64
}

#[cfg(target_os = "macos")]
fn reject_finder_alias(file: &File) -> Result<(), SafePathError> {
    const FINDER_INFO: &str = "com.apple.FinderInfo";
    const ALIAS_FLAG: u16 = 0x8000;
    let mut finder_info = [0_u8; 32];
    let finder_info_length = match fs::fgetxattr(file, FINDER_INFO, &mut finder_info) {
        Ok(length) => length,
        Err(Errno::NOATTR) => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if finder_info_length >= 10
        && u16::from_be_bytes([finder_info[8], finder_info[9]]) & ALIAS_FLAG != 0
    {
        return Err(SafePathError::FinderAlias);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
const fn reject_finder_alias(_file: &File) -> Result<(), SafePathError> {
    Ok(())
}

fn open_error(error: Errno) -> SafePathError {
    if error == Errno::NOENT {
        SafePathError::NotFound
    } else if error == Errno::LOOP {
        SafePathError::SymbolicLink
    } else {
        io_error(error)
    }
}

fn io_error(error: Errno) -> SafePathError {
    SafePathError::Io(error.into())
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs,
        io::Read,
        os::unix::{fs::symlink, net::UnixListener},
    };

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn opens_regular_files_and_directories_from_the_root_handle() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::create_dir(directory.path().join("src"))?;
        fs::write(directory.path().join("src/lib.rs"), b"safe")?;
        let workspace = WorkspaceDirectory::open(directory.path())?;

        let opened_root = workspace.open_directory(".")?;
        let root_stat = rustix::fs::fstat(&opened_root)?;
        assert_eq!(identity(&root_stat), workspace.identity());

        let opened_directory = workspace.open_directory("src")?;
        let directory_stat = rustix::fs::fstat(&opened_directory)?;
        assert!(FileType::from_raw_mode(directory_stat.st_mode).is_dir());

        let mut opened_file = workspace.open_file("src/lib.rs")?;
        let mut content = String::new();
        opened_file.read_to_string(&mut content)?;
        assert_eq!(content, "safe");
        Ok(())
    }

    #[test]
    fn rejects_absolute_parent_and_non_normal_relative_paths() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::write(directory.path().join("file.txt"), b"safe")?;
        let workspace = WorkspaceDirectory::open(directory.path())?;

        for path in [
            "",
            "/etc/passwd",
            "../file.txt",
            "src/../file.txt",
            "./file.txt",
            "src//file.txt",
            "file.txt/",
        ] {
            assert!(matches!(
                workspace.open_file(path),
                Err(SafePathError::InvalidRelativePath)
            ));
        }
        Ok(())
    }

    #[test]
    fn rejects_symlinks_in_final_and_intermediate_components() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let outside = tempdir()?;
        fs::write(outside.path().join("secret.txt"), b"outside")?;
        symlink(
            outside.path().join("secret.txt"),
            directory.path().join("file-link"),
        )?;
        symlink(outside.path(), directory.path().join("directory-link"))?;
        let workspace = WorkspaceDirectory::open(directory.path())?;

        assert!(matches!(
            workspace.open_file("file-link"),
            Err(SafePathError::SymbolicLink)
        ));
        assert!(matches!(
            workspace.open_file("directory-link/secret.txt"),
            Err(SafePathError::SymbolicLink)
        ));
        Ok(())
    }

    #[test]
    fn rejects_a_symlink_as_the_workspace_root() -> Result<(), Box<dyn Error>> {
        let parent = tempdir()?;
        let root = parent.path().join("root");
        let root_link = parent.path().join("root-link");
        fs::create_dir(&root)?;
        symlink(&root, &root_link)?;

        assert!(matches!(
            WorkspaceDirectory::open(root_link),
            Err(SafePathError::SymbolicLink | SafePathError::Io(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_special_files() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let socket_path = directory.path().join("agent.sock");
        let _listener = UnixListener::bind(&socket_path)?;
        let workspace = WorkspaceDirectory::open(directory.path())?;

        assert!(matches!(
            workspace.open_file("agent.sock"),
            Err(SafePathError::UnexpectedFileType)
        ));
        Ok(())
    }

    #[test]
    fn requires_exact_case_and_unicode_normalization() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let decomposed = "Cafe\u{301}.txt";
        let composed = "Caf\u{e9}.txt";
        fs::write(directory.path().join("Case.txt"), b"case")?;
        fs::write(directory.path().join(decomposed), b"unicode")?;
        let workspace = WorkspaceDirectory::open(directory.path())?;

        assert!(workspace.open_file("Case.txt").is_ok());
        assert!(matches!(
            workspace.open_file("case.txt"),
            Err(SafePathError::NotFound)
        ));
        assert!(workspace.open_file(decomposed).is_ok());
        assert!(matches!(
            workspace.open_file(composed),
            Err(SafePathError::NotFound)
        ));
        Ok(())
    }

    #[test]
    fn renamed_root_does_not_authorize_a_recreated_path() -> Result<(), Box<dyn Error>> {
        let parent = tempdir()?;
        let original = parent.path().join("workspace");
        let moved = parent.path().join("moved-workspace");
        fs::create_dir(&original)?;
        fs::write(original.join("trusted.txt"), b"trusted")?;
        let workspace = WorkspaceDirectory::open(&original)?;
        let identity = workspace.identity();

        fs::rename(&original, &moved)?;
        fs::create_dir(&original)?;
        fs::write(original.join("replacement.txt"), b"replacement")?;

        assert_eq!(workspace.identity(), identity);
        assert!(workspace.open_file("trusted.txt").is_ok());
        assert!(matches!(
            workspace.open_file("replacement.txt"),
            Err(SafePathError::NotFound)
        ));
        Ok(())
    }

    #[test]
    fn concurrent_final_component_replacement_fails_closed() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let outside = tempdir()?;
        let target = directory.path().join("target.txt");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&target, b"inside")?;
        fs::write(&outside_file, b"outside")?;
        let workspace = WorkspaceDirectory::open(directory.path())?;
        let mut replacement_error = None;

        let opened = workspace.open_entry(
            Path::new("target.txt"),
            ExpectedType::RegularFile,
            |_, _| {
                if let Err(error) =
                    fs::remove_file(&target).and_then(|()| symlink(&outside_file, &target))
                {
                    replacement_error = Some(error);
                }
            },
        );
        if let Some(error) = replacement_error {
            return Err(error.into());
        }
        assert!(matches!(
            opened,
            Err(SafePathError::SymbolicLink | SafePathError::Io(_))
        ));
        Ok(())
    }

    #[test]
    fn concurrent_directory_replacement_fails_closed() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let outside = tempdir()?;
        let child = directory.path().join("child");
        let moved_child = directory.path().join("moved-child");
        fs::create_dir(&child)?;
        fs::write(child.join("inside.txt"), b"inside")?;
        fs::write(outside.path().join("inside.txt"), b"outside")?;
        let workspace = WorkspaceDirectory::open(directory.path())?;
        let mut replacement_error = None;

        let opened = workspace.open_entry(
            Path::new("child/inside.txt"),
            ExpectedType::RegularFile,
            |index, _| {
                if index == 0 {
                    let replaced = fs::rename(&child, &moved_child)
                        .and_then(|()| symlink(outside.path(), &child));
                    if let Err(error) = replaced {
                        replacement_error = Some(error);
                    }
                }
            },
        );
        if let Some(error) = replacement_error {
            return Err(error.into());
        }
        assert!(matches!(
            opened,
            Err(SafePathError::SymbolicLink | SafePathError::Io(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_mounted_subdirectories_when_the_platform_exposes_one() -> Result<(), Box<dyn Error>>
    {
        use std::os::unix::fs::MetadataExt;

        let root_metadata = fs::metadata("/")?;
        let device_metadata = fs::metadata("/dev")?;
        if root_metadata.dev() != device_metadata.dev() {
            let workspace = WorkspaceDirectory::open("/")?;
            let opened = workspace.open_directory("dev");
            assert!(
                matches!(&opened, Err(SafePathError::CrossDevice)),
                "mounted directory result: {opened:?}"
            );
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_finder_alias_metadata() -> Result<(), Box<dyn Error>> {
        use rustix::fs::XattrFlags;

        let directory = tempdir()?;
        let path = directory.path().join("alias-file");
        fs::write(&path, b"alias")?;
        let file = File::open(&path)?;
        let mut finder_info = [0_u8; 32];
        finder_info[8..10].copy_from_slice(&0x8000_u16.to_be_bytes());
        rustix::fs::fsetxattr(
            &file,
            "com.apple.FinderInfo",
            &finder_info,
            XattrFlags::empty(),
        )?;
        let workspace = WorkspaceDirectory::open(directory.path())?;

        assert!(matches!(
            workspace.open_file("alias-file"),
            Err(SafePathError::FinderAlias)
        ));
        Ok(())
    }
}
