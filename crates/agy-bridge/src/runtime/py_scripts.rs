//! Python initialization script and prompt decoding helpers.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use pyo3::prelude::*;

pub const PYTHON_AGENT_INIT_SCRIPT: &str = include_str!("py/agent_init.py");

/// Decode multimodal prompt content from JSON and map it to Python SDK objects.
pub fn decode_prompt_py<'py>(py: Python<'py>, prompt_str: &str) -> PyResult<Bound<'py, PyAny>> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(prompt_str) {
        decode_content_value(py, &value)
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
) -> PyResult<Bound<'py, PyAny>> {
    match value {
        serde_json::Value::String(s) => Ok(pyo3::types::PyString::new(py, s.as_str()).into_any()),
        serde_json::Value::Array(arr) => {
            let py_list = pyo3::types::PyList::empty(py);
            for item in arr {
                py_list.append(decode_content_value(py, item)?)?;
            }
            Ok(py_list.into_any())
        }
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(typ)) = map.get("type")
                && matches!(typ.as_str(), "Image" | "Document" | "Audio" | "Video")
            {
                let data_b64 = map.get("data").and_then(|v| v.as_str()).unwrap_or("");
                let raw_bytes = STANDARD.decode(data_b64).map_err(|e| {
                    pyo3::exceptions::PyValueError::new_err(format!("Invalid base64: {e}"))
                })?;
                let mime_type = map.get("mime_type").and_then(|v| v.as_str()).unwrap_or("");

                let types_mod = py.import("google.antigravity.types")?;
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
                py_dict.set_item(k, decode_content_value(py, v)?)?;
            }
            Ok(py_dict.into_any())
        }
        _ => {
            // Fallback for null, bool, numbers
            let obj = pythonize::pythonize(py, value).map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!("Serialization failed: {e}"))
            })?;
            Ok(obj)
        }
    }
}
