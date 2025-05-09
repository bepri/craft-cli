use pyo3::{prelude::*, pymodule};

mod craft_cli_utils;
mod printer;
mod test_utils;
mod utils;

/// Formats the sum of two numbers as string.
#[pyfunction]
fn sum_as_string(a: usize, b: usize) -> PyResult<String> {
    Ok((a + b).to_string())
}

/// A Python module implemented in Rust.
#[pymodule]
mod _rs {
    use crate::utils::fix_imports;

    use super::*;

    #[pymodule_export]
    use super::sum_as_string;

    #[pymodule_export]
    use crate::craft_cli_utils::utils;

    #[pymodule_export]
    use crate::printer::printer;

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        fix_imports(m, "craft_cli._rs")
    }
}
