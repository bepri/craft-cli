//! The output (for different destinations) handler and helper functions.

use std::{fs::File, io::Stdout};

use jiff::Timestamp;
use pyo3::{pyclass, pymethods};

const SPINNER_THRESHHOLD: f32 = 0.0;
const SPINNER_DELAY: f32 = 0.001;

struct MessageInfo {
    stream: Option<Stdout>,
    text: String,
    ephemeral: bool,
    bar_progress: Option<f32>,
    bar_total: f32,
    use_timestamp: bool,
    end_line: bool,
    created_at: Timestamp,
    terminal_prefix: String,
}

impl MessageInfo {
    pub fn new(
        stream: Option<Stdout>,
        text: String,
        ephemeral: bool,
        bar_progress: Option<f32>,
        bar_total: f32,
        use_timestamp: bool,
        end_line: bool,
        created_at: Timestamp,
        terminal_prefix: String,
    ) -> Self {
        Self {
            stream,
            text,
            ephemeral,
            bar_progress,
            bar_total,
            use_timestamp,
            end_line,
            created_at,
            terminal_prefix,
        }
    }
}

#[pyclass]
struct Printer {
    stopped: bool,
    prv_msg: MessageInfo,
    log: File,
}

// #[pymethods]
// impl Printer {
//     #[new]
//     pub fn new(log_filepath: String) -> Self {}
// }
