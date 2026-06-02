//! Path normalization and workspace confinement utilities.

use std::path::{Path, PathBuf};

use tracing::warn;

/// Normalize a path by logically resolving `.` and `..` components.
///
/// This function does **not** access the filesystem, so it works for paths
/// that may not exist on disk (e.g. in unit tests). It processes each
/// component in order:
///
/// - [`std::path::Component::CurDir`] (`.`) — skipped
/// - [`std::path::Component::ParentDir`] (`..`) — pops the last pushed
///   component (if any), preventing traversal above the root
/// - All other components — pushed onto the result
///
/// # Security
///
/// This function performs **purely logical** normalization.  It does **not**
/// resolve symbolic links.  If the path contains a symlink component, the
/// normalized result may point to a different location on disk than the
/// logical resolution suggests.
///
/// **For security-sensitive checks** (e.g. workspace confinement), prefer
/// [`canonicalize_path`] which calls [`std::fs::canonicalize`] and resolves
/// symlinks through the real filesystem.  Use this function only as a
/// fallback for paths that may not yet exist on disk.
///
/// # Examples
///
/// ```
/// use std::path::{Path, PathBuf};
///
/// assert_eq!(
///     agy_bridge::policies::normalize_path(Path::new("/workspace/../etc/passwd")),
///     PathBuf::from("/etc/passwd"),
/// );
/// assert_eq!(
///     agy_bridge::policies::normalize_path(Path::new("/workspace/./subdir/file.rs")),
///     PathBuf::from("/workspace/subdir/file.rs"),
/// );
/// ```
#[must_use]
pub fn normalize_path(path: &std::path::Path) -> PathBuf {
    use std::path::Component;

    let mut parts: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => { /* skip '.' */ }
            Component::ParentDir => {
                // Pop only Normal components; never pop RootDir/Prefix.
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }

    parts.iter().collect()
}

/// Resolve a path to its canonical, absolute form via the filesystem.
///
/// Unlike [`normalize_path`], this function accesses the real filesystem and
/// fully resolves symbolic links, `..`, and `.` components.  The returned
/// path contains no symlink segments and is suitable for security-sensitive
/// comparisons such as workspace confinement checks.
///
/// # Errors
///
/// Returns an error if any component of the path does not exist or is not
/// accessible.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// let canon =
///     agy_bridge::policies::canonicalize_path(Path::new("/tmp")).expect("/tmp must exist");
/// assert!(canon.is_absolute());
/// ```
pub fn canonicalize_path(path: &std::path::Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Check whether `candidate` falls under any workspace root.
///
/// When the candidate path exists on disk, both it and each workspace root
/// are resolved through [`canonicalize_path`] (which follows symlinks).
/// If canonicalization fails for either side (e.g. the path does not exist
/// yet), the function falls back to [`normalize_path`] for that operand and
/// logs a warning.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
///
/// let ws = [PathBuf::from("/workspace")];
/// assert!(agy_bridge::policies::is_path_in_workspace(
///     "/workspace/src/main.rs",
///     &ws
/// ));
/// assert!(!agy_bridge::policies::is_path_in_workspace(
///     "/workspace/../etc/passwd",
///     &ws
/// ));
/// ```
#[must_use]
pub fn is_path_in_workspace(candidate: impl AsRef<Path>, workspaces: &[PathBuf]) -> bool {
    let candidate_path = candidate.as_ref();
    let resolved_candidate = match canonicalize_path(candidate_path) {
        Ok(p) => p,
        Err(e) => {
            warn!(
                "canonicalize failed for candidate {candidate_path:?}, \
                 falling back to logical normalization: {e}"
            );
            normalize_path(candidate_path)
        }
    };

    workspaces.iter().any(|ws| {
        let resolved_ws = match canonicalize_path(ws) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "canonicalize failed for workspace {ws:?}, \
                     falling back to logical normalization: {e}"
                );
                normalize_path(ws)
            }
        };
        resolved_candidate.starts_with(&resolved_ws)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_path_in_workspace_accepts_path() {
        let workspaces = [PathBuf::from("/workspace")];
        let p = std::path::Path::new("/workspace/src/main.rs");
        assert!(is_path_in_workspace(p, &workspaces));
    }

    #[test]
    fn is_path_in_workspace_accepts_pathbuf() {
        let workspaces = [PathBuf::from("/workspace")];
        let p = PathBuf::from("/workspace/src/main.rs");
        assert!(is_path_in_workspace(p, &workspaces));
    }
}
