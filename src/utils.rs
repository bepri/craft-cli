//! Internal utils for Craft CLI.

use pyo3::{
    types::{PyAnyMethods, PyModule},
    Bound, PyResult, Python,
};

/// Hack: workaround for [an upstream issue in PyO3](https://github.com/PyO3/pyo3/issues/759)
pub fn fix_imports(m: &Bound<'_, PyModule>, name: &str) -> PyResult<()> {
    Python::with_gil(|py| py.import("sys")?.getattr("modules")?.set_item(name, m))
}
