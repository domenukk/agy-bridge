//! Virtual environment discovery and Python sys path configuration.

use pyo3::prelude::*;

/// Configure Python's `sys.path` to include the virtual environment's
/// `site-packages`, and set `ANTIGRAVITY_HARNESS_PATH` if found.
///
/// We avoid `site.getsitepackages()` because on Debian/Ubuntu systems it
/// returns `dist-packages` paths that don't match the venv layout.
pub(crate) fn configure_python_sys_path(py: Python<'_>) -> PyResult<()> {
    let sys = py.import("sys")?;

    // NOLINT: `.ok()` — env var not set is expected; falls back to cwd below
    let workspace_root = match std::env::var("CARGO_MANIFEST_DIR").ok() {
        Some(dir) if !dir.is_empty() => discover_venv_root(std::path::Path::new(&dir)),
        _ => {
            tracing::warn!(
                "CARGO_MANIFEST_DIR not set or empty, falling back to current directory for venv discovery"
            );
            std::env::current_dir().map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "Failed to determine current directory for venv discovery: {e}"
                ))
            })?
        }
    };

    let venv = workspace_root.join(".venv");
    tracing::debug!(
        workspace_root = %workspace_root.display(),
        venv = %venv.display(),
        venv_exists = venv.is_dir(),
        "Runtime thread: venv discovery"
    );

    if !venv.is_dir() {
        return Ok(());
    }

    let os = py.import("os")?;
    let environ = os.getattr("environ")?;
    environ.set_item("VIRTUAL_ENV", venv.to_string_lossy().to_string())?;
    tracing::debug!(path = %venv.display(), "Set VIRTUAL_ENV in Python os.environ");

    // Extract Python major.minor version
    let version_info = sys.getattr("version_info")?;
    let major: u32 = version_info.getattr("major")?.extract()?;
    let minor: u32 = version_info.getattr("minor")?.extract()?;
    let py_version = format!("{major}.{minor}");

    let unix_site_packages = venv
        .join("lib")
        .join(format!("python{py_version}"))
        .join("site-packages");
    let win_site_packages = venv.join("Lib").join("site-packages");
    let site_packages = if unix_site_packages.is_dir() {
        unix_site_packages
    } else {
        win_site_packages
    };

    #[cfg(windows)]
    let harness_bin = "localharness.exe";
    #[cfg(not(windows))]
    let harness_bin = "localharness";

    // Set ANTIGRAVITY_HARNESS_PATH if the binary exists.
    let harness_path = site_packages
        .join("google")
        .join("antigravity")
        .join("bin")
        .join(harness_bin);

    if harness_path.is_file() {
        environ.set_item(
            "ANTIGRAVITY_HARNESS_PATH",
            harness_path.to_string_lossy().to_string(),
        )?;
        tracing::debug!(path = %harness_path.display(), "Set ANTIGRAVITY_HARNESS_PATH in Python os.environ");
    }

    // Use site.addsitedir() to add venv site-packages. Unlike a plain
    // sys.path.insert(), addsitedir() processes .pth files — which is
    // required for editable (pip install -e) packages that rely on
    // dynamic finder hooks installed via .pth import statements.
    if site_packages.is_dir() {
        let sp_str = site_packages.to_string_lossy().to_string();
        let site_mod = py.import("site")?;
        site_mod.call_method1("addsitedir", (sp_str.as_str(),))?;
        let sys_path = sys.getattr("path")?;
        sys_path.call_method1("insert", (0, sp_str.as_str()))?;
        tracing::debug!(path = %sp_str, "Added venv site-packages via site.addsitedir() and sys.path.insert(0)");
    }

    verify_python_sdk_version(py)?;

    Ok(())
}

/// Minimum supported version of `google-antigravity`.
pub const MIN_SUPPORTED_SDK_VERSION: &str = "0.1.16";

/// Parses a semver-like version string (e.g. "0.1.16" or "0.1.16.post1") into `(major, minor, patch)`.
pub fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = match parts.next()?.parse::<u64>() {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(error = %err, version, "failed to parse major version");
            return None;
        }
    };
    let minor = match parts.next()?.parse::<u64>() {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(error = %err, version, "failed to parse minor version");
            return None;
        }
    };
    let patch_part = parts.next()?.split(['-', '+', 'a', 'b', 'r']).next()?;
    let patch = match patch_part.parse::<u64>() {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(error = %err, version, "failed to parse patch version");
            return None;
        }
    };
    Some((major, minor, patch))
}

/// Checks whether `installed` version satisfies the `required` version.
pub fn is_version_compatible(installed: &str, required: &str) -> bool {
    match (parse_semver(installed), parse_semver(required)) {
        (Some(inst), Some(req)) => inst >= req,
        _ => false,
    }
}

/// Verifies that the installed `google-antigravity` Python package satisfies the
/// minimum supported version requirements.
pub(crate) fn verify_python_sdk_version(py: Python<'_>) -> PyResult<Option<String>> {
    let importlib_metadata = match py.import("importlib.metadata") {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "importlib.metadata not available in Python environment");
            return Ok(None);
        }
    };

    let version_obj = match importlib_metadata.call_method1("version", ("google-antigravity",)) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "google-antigravity package not installed in environment");
            return Ok(None);
        }
    };

    let version_str: String = version_obj.extract()?;
    if !is_version_compatible(&version_str, MIN_SUPPORTED_SDK_VERSION) {
        let msg = format!(
            "Installed google-antigravity version {version_str} is older than minimum supported version {MIN_SUPPORTED_SDK_VERSION}"
        );
        tracing::error!("{msg}");
        return Err(pyo3::exceptions::PyRuntimeError::new_err(msg));
    }

    tracing::debug!(
        installed = %version_str,
        min_required = MIN_SUPPORTED_SDK_VERSION,
        "google-antigravity version check succeeded"
    );
    Ok(Some(version_str))
}

/// Walk upward from `start` to find the nearest ancestor containing a `.venv`
/// directory. Returns `start` itself if no `.venv` is found.
///
/// This is a pure filesystem function, testable without Python.
pub(crate) fn discover_venv_root(start: &std::path::Path) -> std::path::PathBuf {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".venv").is_dir() {
            return current;
        }
        match current.parent() {
            Some(p) if p != current => current = p.to_path_buf(),
            _ => {
                tracing::debug!(
                    "No .venv found walking up from {}, using start dir",
                    start.display()
                );
                return start.to_path_buf();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_venv_root_finds_venv_in_current_dir() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        std::fs::create_dir(tmp.path().join(".venv")).expect("create .venv");

        let result = discover_venv_root(tmp.path());
        assert_eq!(result, tmp.path());
    }

    #[test]
    fn discover_venv_root_walks_up_to_parent() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        std::fs::create_dir(tmp.path().join(".venv")).expect("create .venv");
        let child = tmp.path().join("crates").join("my-crate");
        std::fs::create_dir_all(&child).expect("create child dirs");

        let result = discover_venv_root(&child);
        assert_eq!(result, tmp.path());
    }

    #[test]
    fn discover_venv_root_falls_back_to_start_when_no_venv() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        // No .venv directory created.
        let result = discover_venv_root(tmp.path());
        assert_eq!(result, tmp.path());
    }

    #[test]
    fn discover_venv_root_stops_at_nearest_venv() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        // Create .venv at root and in a child.
        std::fs::create_dir(tmp.path().join(".venv")).expect("create root .venv");
        let child = tmp.path().join("sub");
        std::fs::create_dir_all(&child).expect("create sub dir");
        std::fs::create_dir(child.join(".venv")).expect("create child .venv");

        // Starting from child, should find child's .venv first.
        let result = discover_venv_root(&child);
        assert_eq!(result, child);
    }

    #[test]
    fn test_parse_semver_valid() {
        assert_eq!(parse_semver("0.1.16"), Some((0, 1, 16)));
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("0.1.16.post1"), Some((0, 1, 16)));
        assert_eq!(parse_semver("0.1.16-alpha"), Some((0, 1, 16)));
    }

    #[test]
    fn test_parse_semver_invalid() {
        assert_eq!(parse_semver("invalid"), None);
        assert_eq!(parse_semver("0.1"), None);
    }

    #[test]
    fn test_is_version_compatible() {
        assert!(is_version_compatible("0.1.16", "0.1.16"));
        assert!(is_version_compatible("0.1.17", "0.1.16"));
        assert!(is_version_compatible("0.2.0", "0.1.16"));
        assert!(is_version_compatible("1.0.0", "0.1.16"));
        assert!(!is_version_compatible("0.1.15", "0.1.16"));
        assert!(!is_version_compatible("0.0.9", "0.1.16"));
        assert!(!is_version_compatible("not-a-version", "0.1.16"));
    }
}
