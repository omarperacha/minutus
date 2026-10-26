//! Platform bookmark creation and access.

use std::path::Path;

#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use objc2::{rc::Retained, runtime::Bool};
#[cfg(target_os = "macos")]
use objc2_foundation::{
    NSData, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
};

/// Creates a durable bookmark for a user-selected folder.
///
/// Signed sandboxed builds use app-scoped security bookmarks. Tauri's ordinary
/// development executable is not sandboxed or entitlement-signed, so it uses
/// a non-security-scoped bookmark that survives ad-hoc rebuilds. The Rust
/// permission broker enforces the same project grant in both environments.
#[cfg(target_os = "macos")]
pub(crate) fn create(path: &Path) -> Result<Vec<u8>, ()> {
    create_with_access(path, false)
}

/// Persists access to exactly one selected model file, with no write authority.
#[cfg(target_os = "macos")]
pub(crate) fn create_read_only(path: &Path) -> Result<Vec<u8>, ()> {
    create_with_access(path, true)
}

#[cfg(target_os = "macos")]
fn create_with_access(path: &Path, read_only: bool) -> Result<Vec<u8>, ()> {
    let url = NSURL::from_file_path(path).ok_or(())?;
    let options = if security_scope_required() {
        let mut options = NSURLBookmarkCreationOptions::WithSecurityScope;
        if read_only {
            options |= NSURLBookmarkCreationOptions::SecurityScopeAllowOnlyReadAccess;
        }
        options
    } else {
        NSURLBookmarkCreationOptions::WithoutImplicitSecurityScope
    };
    url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
        options, None, None,
    )
    .map(|data| data.to_vec())
    .map_err(|_| ())
}

/// Security-scoped bookmarks are supported only by the macOS desktop build.
#[cfg(not(target_os = "macos"))]
pub(crate) const fn create(_path: &Path) -> Result<Vec<u8>, ()> {
    Err(())
}

/// One active security-scoped workspace URL.
#[cfg(target_os = "macos")]
pub(crate) struct SecurityScopedWorkspace {
    url: Retained<NSURL>,
    path: PathBuf,
    security_scope_started: bool,
}

#[cfg(target_os = "macos")]
impl SecurityScopedWorkspace {
    /// Returns the trusted path resolved from the Rust-owned bookmark.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(target_os = "macos")]
impl Drop for SecurityScopedWorkspace {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.security_scope_started {
            // SAFETY: `url` is retained for this guard's full lifetime, and
            // every successful start below creates exactly one matching stop.
            unsafe { self.url.stopAccessingSecurityScopedResource() };
        }
    }
}

/// Failure to activate a stored workspace bookmark.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BookmarkAccessError {
    /// The bookmark could not be decoded into a local file URL.
    Invalid,
    /// The bookmark is stale and requires explicit re-authorization.
    Stale,
    /// macOS refused security-scoped access.
    Denied,
    /// Security-scoped bookmarks are unavailable on this platform.
    #[cfg(not(target_os = "macos"))]
    Unsupported,
}

/// Resolves and activates a stored security-scoped bookmark without UI or mounts.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) fn access(bookmark_data: &[u8]) -> Result<SecurityScopedWorkspace, BookmarkAccessError> {
    if bookmark_data.is_empty() {
        return Err(BookmarkAccessError::Invalid);
    }
    let bookmark_data = NSData::with_bytes(bookmark_data);
    let mut stale = Bool::NO;
    let requires_security_scope = security_scope_required();
    let mut options =
        NSURLBookmarkResolutionOptions::WithoutMounting | NSURLBookmarkResolutionOptions::WithoutUI;
    if requires_security_scope {
        options |= NSURLBookmarkResolutionOptions::WithSecurityScope
            | NSURLBookmarkResolutionOptions::WithoutImplicitStartAccessing;
    }
    // SAFETY: `stale` is a valid writable Objective-C BOOL for the duration of
    // the synchronous call. All other arguments are retained Foundation values.
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &bookmark_data,
            options,
            None,
            &raw mut stale,
        )
    }
    .map_err(|_| BookmarkAccessError::Invalid)?;
    if stale.as_bool() {
        return Err(BookmarkAccessError::Stale);
    }
    let path = url.to_file_path().ok_or(BookmarkAccessError::Invalid)?;
    if !path.is_absolute() {
        return Err(BookmarkAccessError::Invalid);
    }
    let security_scope_started = if requires_security_scope {
        // SAFETY: the URL was produced by resolving a security-scoped bookmark.
        // The returned guard retains it and balances this successful start in Drop.
        if !unsafe { url.startAccessingSecurityScopedResource() } {
            return Err(BookmarkAccessError::Denied);
        }
        true
    } else {
        false
    };
    Ok(SecurityScopedWorkspace {
        url,
        path,
        security_scope_started,
    })
}

/// Sandboxed macOS processes receive this variable from the operating system.
/// Treating a spoofed value as sandboxed only makes access stricter; omitting
/// security scope cannot bypass the kernel sandbox in a production build.
#[cfg(target_os = "macos")]
fn security_scope_required() -> bool {
    std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some()
}

/// Security-scoped bookmarks are unavailable outside macOS.
#[cfg(not(target_os = "macos"))]
pub(crate) const fn access(
    _bookmark_data: &[u8],
) -> Result<SecurityScopedWorkspace, BookmarkAccessError> {
    Err(BookmarkAccessError::Unsupported)
}

/// Uninhabited non-macOS placeholder used only by the unsupported function.
#[cfg(not(target_os = "macos"))]
pub(crate) struct SecurityScopedWorkspace;

#[cfg(not(target_os = "macos"))]
impl SecurityScopedWorkspace {
    pub(crate) fn path(&self) -> &Path {
        Path::new("")
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn empty_bookmarks_fail_before_foundation_resolution() {
        assert!(matches!(access(&[]), Err(BookmarkAccessError::Invalid)));
    }

    #[test]
    fn development_bookmark_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        if security_scope_required() {
            return Ok(());
        }
        let directory =
            std::env::temp_dir().join(format!("zeta-bookmark-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&directory)?;
        let bookmark = create(&directory).map_err(|()| "bookmark creation failed")?;
        let workspace =
            access(&bookmark).map_err(|error| format!("bookmark access failed: {error:?}"))?;
        assert_eq!(workspace.path(), fs::canonicalize(&directory)?);
        drop(workspace);
        fs::remove_dir(&directory)?;
        Ok(())
    }
}
