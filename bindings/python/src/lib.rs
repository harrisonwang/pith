use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString};
use spoor_core::{DocumentFilter, Format, ParseLimits, ParseRequest, ProvenanceLevel, TableFilter};
use std::str::FromStr;

pyo3::create_exception!(_native, SpoorError, PyException);

#[allow(clippy::too_many_arguments)]
#[pyfunction(signature = (data, source_name=None, content_type=None, format=None, max_parse_bytes=None, sheet=None, rows=None, columns=None, limit=None, offset=None, pages=None, max_work_units=None, provenance=None, keep_repeated_regions=None))]
fn parse_bytes(
    py: Python<'_>,
    data: &[u8],
    source_name: Option<&str>,
    content_type: Option<&str>,
    format: Option<&str>,
    max_parse_bytes: Option<usize>,
    sheet: Option<String>,
    rows: Option<(usize, usize)>,
    columns: Option<Vec<String>>,
    limit: Option<usize>,
    offset: Option<usize>,
    pages: Option<(usize, usize)>,
    max_work_units: Option<usize>,
    provenance: Option<&str>,
    keep_repeated_regions: Option<bool>,
) -> PyResult<Py<PyAny>> {
    let mut request = request(
        data,
        source_name,
        content_type,
        format,
        max_parse_bytes,
        max_work_units,
    )?;
    request.table_filter =
        TableFilter::build(sheet, rows, columns.unwrap_or_default(), limit, offset)
            .map_err(to_py_error)?;
    request.document_filter = DocumentFilter::build(pages).map_err(to_py_error)?;
    request.document_filter.keep_repeated_regions = keep_repeated_regions.unwrap_or(false);
    if let Some(level) = provenance {
        request.provenance = ProvenanceLevel::from_str(level).map_err(to_py_error)?;
    }
    let result = py
        .detach(|| spoor_core::parse(&request))
        .map_err(to_py_error)?;
    let value =
        serde_json::to_value(result).map_err(|error| PyException::new_err(error.to_string()))?;
    value_to_python(py, &value)
}

/// Extract one safe embedded media resource referenced by a URI emitted in the
/// parsed output (e.g. `spoor://docx/part/word/media/image1.png`,
/// `spoor://pptx/part/ppt/media/imageN.png`, or `spoor://pdf/obj/{id}/{gen}`).
/// Returns the raw resource bytes; spoor does not decode or interpret them.
#[pyfunction(signature = (data, resource, source_name=None, content_type=None, format=None, max_parse_bytes=None))]
fn extract_media<'py>(
    py: Python<'py>,
    data: &[u8],
    resource: &str,
    source_name: Option<&str>,
    content_type: Option<&str>,
    format: Option<&str>,
    max_parse_bytes: Option<usize>,
) -> PyResult<Bound<'py, PyBytes>> {
    let request = request(
        data,
        source_name,
        content_type,
        format,
        max_parse_bytes,
        None,
    )?;
    let bytes = py
        .detach(|| spoor_core::extract_media(&request, resource))
        .map_err(to_py_error)?;
    Ok(PyBytes::new(py, &bytes))
}

/// Ground an LLM-cited quote in Markdown spoor produced. Returns a dict with
/// the hit's span, collapsed context, source page, and match method, or `None`
/// when no deterministic tier matches — the claim the quote backs is then
/// unverifiable. The span indexes `markdown` as a Python `str`.
#[pyfunction(signature = (markdown, quote, provenance_json=None))]
fn locate_quote(
    py: Python<'_>,
    markdown: &str,
    quote: &str,
    provenance_json: Option<&str>,
) -> PyResult<Py<PyAny>> {
    // The wrapper passes provenance spans as JSON so new anchor kinds flow
    // through without another marshalling layer.
    let spans: Vec<spoor_core::ProvenanceSpan> = match provenance_json {
        Some(json) => serde_json::from_str(json)
            .map_err(|error| SpoorError::new_err(format!("provenance 参数无效：{error}")))?,
        None => Vec::new(),
    };
    let Some(found) = py.detach(|| spoor_core::locate_quote_grounded(markdown, quote, &spans))
    else {
        return Ok(py.None());
    };
    // Core spans are UTF-8 byte offsets; convert to Python str indices so
    // `markdown[start:end]` slices the hit directly.
    let start = markdown[..found.span.start].chars().count();
    let end = start + markdown[found.span.start..found.span.end].chars().count();
    let mut value = serde_json::json!({
        "before": found.before,
        "hit": found.hit,
        "after": found.after,
        "span": {"start": start, "end": end},
        "page": found.page,
        "method": found.method.as_str(),
    });
    if let Some(anchor) = &found.anchor {
        value["anchor"] =
            serde_json::to_value(anchor).map_err(|error| SpoorError::new_err(error.to_string()))?;
    }
    value_to_python(py, &value)
}

#[pyfunction(signature = (data, source_name=None, content_type=None))]
fn detect_format(
    py: Python<'_>,
    data: &[u8],
    source_name: Option<&str>,
    content_type: Option<&str>,
) -> PyResult<String> {
    let request = request(data, source_name, content_type, None, None, None)?;
    py.detach(|| spoor_core::detect_format(&request))
        .map(|format| format.to_string())
        .map_err(to_py_error)
}

fn request<'a>(
    data: &'a [u8],
    source_name: Option<&'a str>,
    content_type: Option<&'a str>,
    format: Option<&str>,
    max_parse_bytes: Option<usize>,
    max_work_units: Option<usize>,
) -> PyResult<ParseRequest<'a>> {
    let mut request = ParseRequest::new(data);
    request.source_name = source_name;
    request.content_type = content_type;
    request.format_hint = format
        .map(Format::from_str)
        .transpose()
        .map_err(to_py_error)?;
    request.limits = ParseLimits {
        max_parse_bytes: max_parse_bytes.unwrap_or(request.limits.max_parse_bytes),
        max_work_units,
    };
    Ok(request)
}

fn to_py_error(error: spoor_core::SpoorError) -> PyErr {
    SpoorError::new_err(error.to_json())
}

fn value_to_python(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(value) => {
            Ok(PyBool::new(py, *value).to_owned().into_any().unbind())
        }
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(PyInt::new(py, value).into_any().unbind())
            } else if let Some(value) = value.as_u64() {
                Ok(PyInt::new(py, value).into_any().unbind())
            } else {
                Ok(PyFloat::new(py, value.as_f64().unwrap_or_default())
                    .into_any()
                    .unbind())
            }
        }
        serde_json::Value::String(value) => Ok(PyString::new(py, value).into_any().unbind()),
        serde_json::Value::Array(values) => {
            let values = values
                .iter()
                .map(|value| value_to_python(py, value))
                .collect::<PyResult<Vec<_>>>()?;
            Ok(PyList::new(py, values)?.into_any().unbind())
        }
        serde_json::Value::Object(values) => {
            let dict = PyDict::new(py);
            for (key, value) in values {
                dict.set_item(key, value_to_python(py, value)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("SpoorError", module.py().get_type::<SpoorError>())?;
    module.add_function(wrap_pyfunction!(parse_bytes, module)?)?;
    module.add_function(wrap_pyfunction!(extract_media, module)?)?;
    module.add_function(wrap_pyfunction!(detect_format, module)?)?;
    module.add_function(wrap_pyfunction!(locate_quote, module)?)?;
    Ok(())
}
