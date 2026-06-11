//! Virtual environment discovery and Python sys path configuration.

use pyo3::prelude::*;

/// Configure Python's `sys.path` to include the virtual environment's
/// `site-packages`, and set `ANTIGRAVITY_HARNESS_PATH` if found.
///
/// We avoid `site.getsitepackages()` because on Debian/Ubuntu systems it
/// returns `dist-packages` paths that don't match the venv layout.
pub(crate) fn configure_python_sys_path(py: Python<'_>) -> PyResult<()> {
    let sys = py.import_bound("sys")?;

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

    let os = py.import_bound("os")?;
    let environ = os.getattr("environ")?;
    environ.set_item("VIRTUAL_ENV", venv.to_string_lossy().to_string())?;
    tracing::debug!(path = %venv.display(), "Set VIRTUAL_ENV in Python os.environ");

    // Extract Python major.minor version
    let version_info = sys.getattr("version_info")?;
    let major: u32 = version_info.getattr("major")?.extract()?;
    let minor: u32 = version_info.getattr("minor")?.extract()?;
    let py_version = format!("{major}.{minor}");

    // Set ANTIGRAVITY_HARNESS_PATH if the binary exists.
    let harness_path = venv
        .join("lib")
        .join(format!("python{py_version}"))
        .join("site-packages")
        .join("google")
        .join("antigravity")
        .join("bin")
        .join("localharness");

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
    let site_packages = venv
        .join("lib")
        .join(format!("python{py_version}"))
        .join("site-packages");

    if site_packages.is_dir() {
        let sp_str = site_packages.to_string_lossy().to_string();
        let site_mod = py.import_bound("site")?;
        site_mod.call_method1("addsitedir", (sp_str.as_str(),))?;
        tracing::debug!(path = %sp_str, "Added venv site-packages via site.addsitedir()");
    }

    Ok(())
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
}
