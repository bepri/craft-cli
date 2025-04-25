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
pub fn get_substring<S>(string: &S, range: Range<usize>) -> PyResult<&str>
where
    S: AsRef<str>,
{
    if range.start > range.end {
        return Err(PyValueError::new_err(
            "Invalid range: start must be <= end.",
        ));
    }

    let string_ref = string.as_ref();
    let mut iter = string_ref.char_indices();

    let (start, _) = iter.nth(range.start).unwrap_or((0, 'a'));
    let (end, _) = iter
        .nth(range.end - range.start - 1)
        .unwrap_or((string_ref.len() - 1, 'z'));

    Ok(&string_ref[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;

    mod get_substring {
        use super::*;

        #[test]
        /// Ensure basic functionality with multiple string types
        fn test_basic() {
            let string = String::from("Hello world!");
            let string_ref = &string;
            let str_ref = string.as_str();

            assert_eq!(get_substring(&string, 0..5).unwrap(), "Hello");
            assert_eq!(get_substring(&string_ref, 0..5).unwrap(), "Hello");
            assert_eq!(get_substring(&str_ref, 0..5).unwrap(), "Hello");
        }

        #[test]
        /// Check that invalid ranges aren't supported
        fn test_errors() {
            let string = String::from("Hello world!");

            #[allow(clippy::reversed_empty_ranges)]
            let err = get_substring(&string, 1..0)
                .expect_err("An error should be raised for bad ranges.");

            tests::assert_error_type::<PyValueError>(&err);
            tests::assert_error_contents(&err, r"Invalid range: start must be <= end\.");
        }
    }
}
