//! `PyO3` pre-tool-call hook for workspace confinement.
// pyo3::pymethods proc-macro generates PyErr→PyErr conversions from `?`
#![allow(clippy::useless_conversion)]

use std::path::PathBuf;

use pyo3::types::{PyAnyMethods, PyDictMethods};

use super::path::is_path_in_workspace;

const HOOKS_MODULE_PATH: &str = "google.antigravity.hooks.hooks";
const HOOK_RESULT_CLASS: &str = "HookResult";
const ARGS_ATTR_NAME: &str = "args";
const PATH_KEYS_TO_CHECK: [&str; 3] = ["path", "file_path", "dir_path"];
const ALLOW_ATTR: &str = "allow";
const MESSAGE_ATTR: &str = "message";

#[pyo3::pyclass(unsendable)]
#[derive(Clone, Debug)]
pub struct PreToolCallDecideHook {
    workspaces: Vec<PathBuf>,
}

#[pyo3::pymethods]
impl PreToolCallDecideHook {
    #[new]
    #[must_use]
    pub fn new(workspaces: Vec<PathBuf>) -> Self {
        Self { workspaces }
    }

    pub fn __call__<'py>(
        &self,
        py: pyo3::Python<'py>,
        ctx: &pyo3::Bound<'py, pyo3::PyAny>,
    ) -> pyo3::PyResult<pyo3::Bound<'py, pyo3::PyAny>> {
        let args: pyo3::Bound<'py, pyo3::types::PyDict> = ctx.getattr(ARGS_ATTR_NAME)?.extract()?;

        let mut paths_to_check = Vec::new();
        for key in PATH_KEYS_TO_CHECK {
            if let Ok(Some(val)) = args.get_item(key)
                && let Ok(s) = val.extract::<String>()
            {
                paths_to_check.push(s);
            }
        }

        for p in paths_to_check {
            if !is_path_in_workspace(&p, &self.workspaces) {
                let hooks_mod = py.import_bound(HOOKS_MODULE_PATH)?;
                let hook_result_cls = hooks_mod.getattr(HOOK_RESULT_CLASS)?;
                let kwargs = pyo3::types::PyDict::new_bound(py);
                kwargs.set_item(ALLOW_ATTR, false)?;
                kwargs.set_item(
                    MESSAGE_ATTR,
                    format!("Path '{p}' is outside permitted workspaces"),
                )?;
                return hook_result_cls.call((), Some(&kwargs));
            }
        }

        let hooks_mod = py.import_bound(HOOKS_MODULE_PATH)?;
        let hook_result_cls = hooks_mod.getattr(HOOK_RESULT_CLASS)?;
        let kwargs = pyo3::types::PyDict::new_bound(py);
        kwargs.set_item(ALLOW_ATTR, true)?;
        hook_result_cls.call((), Some(&kwargs))
    }
}
