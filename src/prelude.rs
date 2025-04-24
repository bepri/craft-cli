use pyo3::PyErrArguments;

pub type Result<T> = std::result::Result<T, Box<dyn PyErrArguments>>;
