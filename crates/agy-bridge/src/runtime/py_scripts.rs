//! Python initialization script and prompt decoding helpers.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use pyo3::prelude::*;

/// Maximum allowed nesting depth when decoding JSON content values.
/// Prevents stack overflow on pathologically nested payloads.
pub(crate) const MAX_DECODE_DEPTH: usize = 64;

pub const PYTHON_AGENT_INIT_SCRIPT: &str = include_str!("py/agent_init.py");

/// Import `module`, serialising its *first* (uncached) import across threads.
///
/// Importing a heavy package such as `google.antigravity.types` executes the
/// module's top-level code, which pulls in the compiled `pydantic` core and
/// releases/re-acquires the GIL partway through. If two threads race the very
/// first import, one can end up parked inside `CPython`'s import machinery while
/// holding the GIL, starving the thread that must finish the import — a hard
/// deadlock. This was observed when the test harness runs SDK-touching tests in
/// parallel, and is equally possible in production when several bridges start at
/// once.
///
/// Once a module is in `sys.modules`, re-importing is a cheap cached lookup that
/// cannot deadlock, so only the first import is serialised. Threads that lose
/// the race wait for the leader with the GIL *released* (via [`Python::detach`]),
/// so a waiter never blocks the importing thread — which is what makes this
/// deadlock-free regardless of whether the caller already holds the GIL.
pub(crate) fn import_serialized<'py>(py: Python<'py>, module: &str) -> PyResult<Bound<'py, PyAny>> {
    static FIRST_IMPORT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // Fast path: already imported — a cached lookup, safe to run concurrently.
    if is_in_sys_modules(py, module)? {
        return py.import(module).map(Bound::into_any);
    }

    // Slow path: serialise the first import. The GIL is released while
    // contending for the lock, so the leader — which needs the GIL to run the
    // import — is never blocked by a waiter that holds the GIL.
    py.detach(|| {
        let _guard = FIRST_IMPORT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            // Re-check under the lock: the leader imports once; stragglers that
            // take the lock afterwards find it cached and skip the work.
            match is_in_sys_modules(py, module) {
                Ok(true) => {}
                Ok(false) => {
                    if let Err(e) = py.import(module) {
                        // Surface but don't panic: the re-import below returns
                        // the genuine error to the caller.
                        tracing::debug!(module, error = %e, "serialized first import failed");
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        module,
                        error = %e,
                        "sys.modules probe failed during serialized import",
                    );
                }
            }
        });
    });

    // Return the now-cached module (or propagate a genuine ImportError).
    py.import(module).map(Bound::into_any)
}

/// Returns whether `module` is already present in `sys.modules`.
fn is_in_sys_modules(py: Python<'_>, module: &str) -> PyResult<bool> {
    py.import("sys")?.getattr("modules")?.contains(module)
}

/// Eagerly import the Python modules that pyo3 / pythonize import *lazily* the
/// first time they inspect an object's abstract base classes — notably
/// `collections.abc`, used for the `Sequence` / `Mapping` checks during
/// (de)serialization.
///
/// Routed through [`import_serialized`] so the very first import is serialised
/// across threads (see that function for the deadlock rationale). Cheap after
/// the first call, so it is safe to call at every pyo3 (de)serialization
/// boundary.
pub(crate) fn warm_up_lazy_imports(py: Python<'_>) {
    // Never silently ignore: if this import fails, later conversions could still
    // race, so make the failure visible in the logs.
    if let Err(e) = import_serialized(py, "collections.abc") {
        tracing::warn!(
            error = %e,
            "failed to pre-import collections.abc during Python warm-up",
        );
    }
}

/// Decode multimodal prompt content from JSON and map it to Python SDK objects.
pub fn decode_prompt_py<'py>(py: Python<'py>, prompt_str: &str) -> PyResult<Bound<'py, PyAny>> {
    // NOLINT: plain-string fallback is intentional when JSON parse fails
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(prompt_str) {
        decode_content_value(py, &value, 0)
    } else {
        // Fallback: treat as a simple string prompt
        Ok(pyo3::types::PyString::new(py, prompt_str).into_any())
    }
}

/// Convert a Pydantic model or python object to a dictionary if possible.
pub fn to_dict_py<'py>(ob: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    if ob.hasattr("model_dump")? {
        ob.call_method0("model_dump")
    } else if ob.hasattr("dict")? {
        ob.call_method0("dict")
    } else {
        Ok(ob.clone())
    }
}

fn decode_content_value<'py>(
    py: Python<'py>,
    value: &serde_json::Value,
    depth: usize,
) -> PyResult<Bound<'py, PyAny>> {
    if depth > MAX_DECODE_DEPTH {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "JSON nesting depth exceeded maximum allowed depth",
        ));
    }
    match value {
        serde_json::Value::String(s) => Ok(pyo3::types::PyString::new(py, s.as_str()).into_any()),
        serde_json::Value::Array(arr) => {
            let py_list = pyo3::types::PyList::empty(py);
            for item in arr {
                py_list.append(decode_content_value(py, item, depth + 1)?)?;
            }
            Ok(py_list.into_any())
        }
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(typ)) = map.get("type")
                && matches!(typ.as_str(), "Image" | "Document" | "Audio" | "Video")
            {
                let data_b64 = map.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err(format!(
                        "{typ} content is missing required 'data' field"
                    ))
                })?;
                let raw_bytes = STANDARD.decode(data_b64).map_err(|e| {
                    pyo3::exceptions::PyValueError::new_err(format!("Invalid base64: {e}"))
                })?;
                let mime_type = map
                    .get("mime_type")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        pyo3::exceptions::PyValueError::new_err(format!(
                            "{typ} content is missing required 'mime_type' field"
                        ))
                    })?;

                let types_mod = import_serialized(py, "google.antigravity.types")?;
                let kwargs = pyo3::types::PyDict::new(py);
                kwargs.set_item("data", pyo3::types::PyBytes::new(py, &raw_bytes))?;
                kwargs.set_item("mime_type", mime_type)?;
                if let Some(serde_json::Value::String(desc)) = map.get("description") {
                    kwargs.set_item("description", desc)?;
                }
                let obj = types_mod.getattr(typ.as_str())?.call((), Some(&kwargs))?;
                return Ok(obj);
            }
            // Fallback: convert dict to Python dict
            let py_dict = pyo3::types::PyDict::new(py);
            for (k, v) in map {
                py_dict.set_item(k, decode_content_value(py, v, depth + 1)?)?;
            }
            Ok(py_dict.into_any())
        }
        _ => {
            // Fallback for null, bool, numbers
            warm_up_lazy_imports(py);
            let obj = pythonize::pythonize(py, value).map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!("Serialization failed: {e}"))
            })?;
            Ok(obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_content_value_string() {
        Python::attach(|py| {
            let result = decode_prompt_py(py, "hello").unwrap();
            let s: String = result.extract().unwrap();
            assert_eq!(s, "hello");
        });
    }

    #[test]
    fn decode_content_value_json_string() {
        Python::attach(|py| {
            let result = decode_prompt_py(py, r#""hello world""#).unwrap();
            let s: String = result.extract().unwrap();
            assert_eq!(s, "hello world");
        });
    }

    #[test]
    fn decode_content_value_array() {
        Python::attach(|py| {
            let result = decode_prompt_py(py, r#"["a", "b"]"#).unwrap();
            let list = result.cast::<pyo3::types::PyList>().unwrap();
            assert_eq!(list.len(), 2);
        });
    }

    #[test]
    fn decode_content_value_depth_limit() {
        // Build a deeply nested JSON array: [[[[...]]]]
        let depth = MAX_DECODE_DEPTH + 10;
        let mut json = String::new();
        for _ in 0..depth {
            json.push('[');
        }
        json.push_str("\"leaf\"");
        for _ in 0..depth {
            json.push(']');
        }
        Python::attach(|py| {
            let result = decode_prompt_py(py, &json);
            assert!(result.is_err(), "should fail with depth exceeded");
            let err_str = format!("{}", result.unwrap_err());
            assert!(
                err_str.contains("nesting") || err_str.contains("depth"),
                "error should mention depth: {err_str}"
            );
        });
    }

    #[test]
    fn decode_content_value_plain_text_fallback() {
        // Not valid JSON — should fall back to plain string
        Python::attach(|py| {
            let result = decode_prompt_py(py, "not json { at all").unwrap();
            let s: String = result.extract().unwrap();
            assert_eq!(s, "not json { at all");
        });
    }
}
