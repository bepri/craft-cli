#![warn(
    clippy::pedantic,
    clippy::mem_forget,
    clippy::allow_attributes,
    clippy::dbg_macro,
    clippy::clone_on_ref_ptr,
    clippy::missing_docs_in_private_items
)]
// Specifically allow wildcard imports as they are a very common pattern for enum
// matching and module setup
#![allow(clippy::wildcard_imports, clippy::enum_glob_use)]

//! Craft CLI
//!
//! The perfect foundation for your CLI situation.

use pyo3::{prelude::*, pymodule};

mod craft_cli_utils;
mod emitter;
mod printer;
mod test_utils;
mod utils;

/// A Python module implemented in Rust.
#[pymodule]
mod _rs {
    use crate::utils::fix_imports;

    use super::*;

    #[pymodule_export]
    use crate::craft_cli_utils::utils;

    /// Fix syspath for easier importing in Python.
    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        fix_imports(m, "craft_cli._rs")
    }
}
