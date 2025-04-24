use std::ops::Range;

use pyo3::{
    exceptions::PyValueError,
    types::{PyAnyMethods, PyModule},
    Bound, PyResult, Python,
};

/// Hack: workaround for https://github.com/PyO3/pyo3/issues/759
pub fn fix_imports(m: &Bound<'_, PyModule>, name: &str) -> PyResult<()> {
    Python::with_gil(|py| py.import("sys")?.getattr("modules")?.set_item(name, m))
}

/// Extract a substring from a range.
pub fn get_substring(string: &str, range: Range<usize>) -> PyResult<&str> {
    if range.start > range.end {
        return Err(PyValueError::new_err(
            "Invalid range: start must be <= end.",
        ));
    }

    let mut iter = string.char_indices();

    let (start, _) = iter.nth(range.start).unwrap_or((0, 'a'));
    let (end, _) = iter
        .nth(range.end - range.start - 1)
        .unwrap_or((string.len() - 1, 'z'));

    Ok(&string[start..end])
}
