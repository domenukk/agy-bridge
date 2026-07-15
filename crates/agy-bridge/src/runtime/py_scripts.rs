//! Python initialization script and prompt decoding helpers.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use pyo3::prelude::*;

/// Maximum allowed nesting depth when decoding JSON content values.
/// Prevents stack overflow on pathologically nested payloads.
pub(crate) const MAX_DECODE_DEPTH: usize = 64;

pub const PYTHON_AGENT_INIT_SCRIPT: &str = include_str!("py/agent_init.py");

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
                py_dict.set_item(k, decode_content_value(py, v, depth + 1)?)?;
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
