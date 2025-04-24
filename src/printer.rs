//! The output (for different destinations) handler and helper functions.

use std::{
    fmt::Display,
    fs::File,
    io::Write,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
};

use jiff::Timestamp;
use lazy_static::lazy_static;
use pyo3::{
    exceptions::PyOSError, pyclass, pymethods, pymodule, types::PyAnyMethods, FromPyObject,
    PyResult,
};

use crate::utils::get_substring;

const SPINNER_THRESHHOLD: f32 = 0.0;
const SPINNER_DELAY: f32 = 0.001;

const ANSI_CLEAR_LINE_TO_END: &str = "\u{001b}[K";
const ANSI_HIDE_CURSOR: &str = "\u{001b}[?25l";
const ANSI_SHOW_CURSOR: &str = "\u{001b}[?25h";

const EXPECT_NO_POISON: &str =
    "Only fails if a panic occurs in another thread, which would be irrecoverable anyways.";

struct Spinner {}

#[derive(Clone)]
struct FileHandle(Option<Arc<Mutex<File>>>);

impl FileHandle {
    /// Convert a handle into its raw FD form
    fn as_raw_fd(&self) -> Option<RawFd> {
        match self.0.as_ref() {
            None => None,
            Some(handle) => match handle.lock() {
                Err(_) => None,
                Ok(handle) => Some(handle.as_raw_fd()),
            },
        }
    }

    pub fn write<S>(&mut self, message: S) -> PyResult<()>
    where
        S: AsRef<[u8]>,
    {
        if let Some(ref mut stream_lock) = self.0 {
            let mut stream = stream_lock.lock().expect(EXPECT_NO_POISON);
            stream.write_all(message.as_ref())?;
        }

        Ok(())
    }

    pub fn flush(&mut self) -> PyResult<()> {
        if let Some(ref mut stream_lock) = self.0 {
            let mut stream = stream_lock.lock().expect(EXPECT_NO_POISON);
            stream.flush()?;
        }

        Ok(())
    }

    pub fn is_writable(&self) -> bool {
        self.0.is_some()
    }
}

impl PartialEq for FileHandle {
    /// Compare whether two streams point to the same handle
    fn eq(&self, other: &Self) -> bool {
        self.as_raw_fd() == other.as_raw_fd()
    }
}

impl FromPyObject<'_> for FileHandle {
    fn extract_bound(ob: &pyo3::Bound<'_, pyo3::PyAny>) -> PyResult<Self> {
        let raw_fd = match ob.extract()? {
            Some(raw_fd) => raw_fd,
            None => return Ok(Self(None)),
        };

        // SAFETY: This function is only safe when accessed via a Python interface.
        // The consuming Python script manages the file descriptor handed here, and
        // it is up to said script to ensure the existence of the raw FD.
        let handle = unsafe { File::from_raw_fd(raw_fd) };

        Ok(Self(Some(Arc::new(Mutex::new(handle)))))
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        if let Some(arc_handle) = self.0.take() {
            if let Some(handle) = Arc::into_inner(arc_handle) {
                _ = handle.into_inner().expect(EXPECT_NO_POISON).into_raw_fd();
            }
        }
    }
}

#[derive(Clone)]
struct TermPrefix(Arc<RwLock<String>>);

impl TermPrefix {
    pub fn new(prefix: &str) -> Self {
        Self(Arc::new(RwLock::new(prefix.to_string())))
    }
    pub fn read(&self) -> RwLockReadGuard<'_, String> {
        self.0.read().expect(EXPECT_NO_POISON)
    }

    pub fn write(&mut self, val: &str) {
        *self.0.write().expect(EXPECT_NO_POISON) = val.to_string();
    }

    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }
}

impl PartialEq for TermPrefix {
    fn eq(&self, other: &Self) -> bool {
        self.read().as_str() == other.read().as_str()
    }
}

impl<S> PartialEq<S> for TermPrefix
where
    S: AsRef<str>,
{
    fn eq(&self, other: &S) -> bool {
        self.read().as_str() == other.as_ref()
    }
}

impl Display for TermPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.read())
    }
}

#[derive(PartialEq, Clone)]
struct MessageInfo {
    stream: FileHandle,
    text: String,
    ephemeral: bool,
    bar_progress: Option<f32>,
    bar_total: Option<f32>,
    use_timestamp: bool,
    end_line: bool,
    created_at: Timestamp,
    terminal_prefix: TermPrefix,
}

#[pyclass]
struct Printer {
    stopped: bool,
    prv_msg: Option<MessageInfo>,
    log: File,
    terminal_prefix: TermPrefix,
    secrets: Vec<String>,
    spinner: Spinner,
}

#[pymethods]
impl Printer {
    #[new]
    pub fn new(log_filepath: PathBuf) -> PyResult<Self> {
        let log = match File::options()
            .truncate(true)
            .create(true)
            .write(true)
            .open(log_filepath)
        {
            Ok(l) => l,
            Err(e) => return Err(PyOSError::new_err(e)),
        };

        Ok(Self {
            stopped: false,
            prv_msg: None,
            log,
            terminal_prefix: TermPrefix::new(""),
            secrets: Vec::new(),
            spinner: Spinner {},
        })
    }

    /// Set the string to be prepended to every message shown to the terminal.
    pub fn set_terminal_prefix(&mut self, prefix: String) {
        self.terminal_prefix.write(&prefix);
    }

    /// Show a text to the given stream if not stopped.
    #[pyo3(signature = (stream, text, *, ephemeral = false, use_timestamp = false, end_line = false, avoid_logging = false))]
    pub fn show(
        &mut self,
        stream: FileHandle,
        text: String,
        ephemeral: bool,
        use_timestamp: bool,
        end_line: bool,
        avoid_logging: bool,
    ) -> PyResult<()> {
        if self.stopped {
            return Ok(());
        }

        let mut msg = MessageInfo {
            stream,
            text: text.trim_end().into(),
            ephemeral,
            use_timestamp,
            bar_progress: None,
            bar_total: None,
            end_line,
            created_at: Timestamp::now(),
            terminal_prefix: self.terminal_prefix.clone(),
        };

        self.show_inner(&mut msg)?;

        if !avoid_logging {
            self.log(&mut msg)?;
        }

        Ok(())
    }

    /// Stop the printing infrastructure.
    ///
    /// In detail:
    /// - stop the spinner
    /// - show the cursor
    /// - add a new line to the screen (if needed)
    /// - close the log file
    pub fn stop(&mut self) -> PyResult<()> {
        if let Some(ref mut prv_msg) = self.prv_msg {
            prv_msg.stream.write("\n")?;
            prv_msg.stream.flush()?;
        }

        self.stopped = true;

        Ok(())
    }
}

impl Printer {
    /// Get the message's text with the proper terminal prefix, if any.
    fn get_prefixed_message_text(&self, message: &MessageInfo) -> String {
        let mut text = message.text.clone();
        let prefix = message.terminal_prefix.clone();

        // Don't repeat text: can happen due to the spinner.
        if !prefix.is_empty() && prefix != text {
            let mut separator = ":: ";

            // Don't duplicate the separator, which can come from multiple different
            // sources.
            if text.starts_with(separator) {
                separator = ""
            }

            text = format!("{} {}{}", prefix, separator, text);
        }

        text
    }

    /// Write a simple line message to the screen.
    fn write_line_terminal(&mut self, message: &mut MessageInfo, spintext: String) -> PyResult<()> {
        // Prepare the text with (maybe) the timestamp and remove trailing spaces
        let mut text = self.get_prefixed_message_text(message).trim_end().into();

        if message.use_timestamp {
            text = format!("{} {}", Self::build_timestamp(&message.created_at), text);
        }

        // Get the end of line to use when writing a line to the terminal.
        let mut previous_line_end = {
            if let Some(prv_msg) = &self.prv_msg {
                if !spintext.is_empty() {
                    // Forced to overwrite the previous message to present the spinner
                    "\r"
                } else if prv_msg.end_line {
                    // Previous message completed the line -- so, start clean
                    ""
                } else if prv_msg.ephemeral {
                    // The last one was ephemeral, so overwrite it
                    "\r"
                } else {
                    // The previous line was ended; complete it
                    "\n"
                }
            } else {
                // First message, nothing special needed
                ""
            }
        };

        if let Some(ref mut prv_msg) = &mut self.prv_msg {
            if prv_msg.ephemeral && prv_msg.stream != message.stream {
                // If the last message's stream is different from this new one,
                // send a carriage return to the original stream only.
                prv_msg.stream.write("\r")?;
                prv_msg.stream.flush()?;
                previous_line_end = "";
            }

            if previous_line_end == "\n" {
                prv_msg.stream.write("\n")?;
                prv_msg.stream.flush()?;
                previous_line_end = "";
            }
        }

        let width = Self::get_terminal_width();

        let usable = width - (spintext.len() - 1);

        if text.len() > usable {
            // Helper function for truncating overflow text length
            let truncate_text = |text: &mut String| -> PyResult<String> {
                Ok(format!("{}…", get_substring(text, 0..text.len() - 1)?))
            };

            if message.ephemeral {
                truncate_text(&mut text)?;
            } else if !spintext.is_empty() {
                // We need to rewrite the message with the spintext, use only the last line for
                // multiline messages, and ensure (again) that the last real line fits
                let remaining_for_last_line = text.len() % width;
                text =
                    get_substring(&text, text.len() - remaining_for_last_line..text.len())?.into();
                if text.len() > usable {
                    truncate_text(&mut text)?;
                }
            }
        }

        if !spintext.is_empty()
            || message.end_line
            || !message.ephemeral
            || Some(&message) != self.prv_msg.as_mut().as_ref()
        {
            let line =
                Self::format_term_line(previous_line_end, text, spintext, message.ephemeral)?;
            message.stream.write(line)?;
            message.stream.flush()?;
        }

        if message.end_line {
            // Finish just the shown line, as we need a clean terminal for external things
            message.stream.write("\n")?;
            message.stream.flush()?;
        }

        Ok(())
    }

    /// Show the composed message.
    fn show_inner(&mut self, message: &mut MessageInfo) -> PyResult<()> {
        if !message.stream.is_writable() {
            return Ok(());
        }

        if message.bar_progress.is_none() {
            self.write_line_terminal(message, "".into())?;
        }

        self.prv_msg = Some(message.clone());
        Ok(())
    }

    /// Write the line message to the log file.
    fn log(&mut self, message: &mut MessageInfo) -> PyResult<()> {
        self.log.write_all(
            format!(
                "{} {}\n",
                Self::build_timestamp(&Timestamp::now()),
                message.text
            )
            .as_ref(),
        )?;
        self.log.flush()?;

        Ok(())
    }

    fn get_terminal_width() -> usize {
        termsize::get().map(|sizes| sizes.cols).unwrap_or(80).into()
    }

    /// Format a line to print to the terminal.
    fn format_term_line(
        previous_line_end: &str,
        mut text: String,
        spintext: String,
        ephemeral: bool,
    ) -> PyResult<String> {
        let width = Self::get_terminal_width();

        let usable = width - (spintext.len() - 1);

        if text.len() > usable {
            // Helper function for truncating overflow text length
            let truncate_text = |text: &mut String| -> PyResult<String> {
                Ok(format!("{}…", get_substring(text, 0..text.len() - 1)?))
            };

            if ephemeral {
                truncate_text(&mut text)?;
            } else if !spintext.is_empty() {
                // We need to rewrite the message with the spintext, use only the last line for
                // multiline messages, and ensure (again) that the last real line fits
                let remaining_for_last_line = text.len() % width;
                text =
                    get_substring(&text, text.len() - remaining_for_last_line..text.len())?.into();
                if text.len() > usable {
                    truncate_text(&mut text)?;
                }
            }
        }

        // Turn the input text into a line that will fill the terminal.
        let line_fill = {
            let line = format!("{}{}", text, spintext);
            match Self::supports_ansi_escape_sequences() {
                true => format!("{}{}", line, ANSI_CLEAR_LINE_TO_END),
                false => {
                    let n_spaces = width - line.len() % width - 1;
                    format!("{: >width$}", line, width = n_spaces)
                }
            }
        };

        Ok(format!("{}{}", previous_line_end, line_fill))
    }

    /// Whether the current environment supports ANSI escape sequences.
    fn supports_ansi_escape_sequences() -> bool {
        // Cache the result in a static variable.
        lazy_static! {
            static ref SUPPORT_PRESENT: bool = {
                // Always true if not on Windows
                #[cfg(not(target_os="windows"))]
                {true}
                // Can be true if in a Windows Terminal session
                #[cfg(target_os="windows")]
                {::std::env::var("WT_SESSION").is_ok()}
            };
        }

        *SUPPORT_PRESENT
    }

    fn build_timestamp(ts: &Timestamp) -> jiff::fmt::strtime::Display<'_> {
        ts.strftime("%F %T%.3f")
    }
}

#[pymodule(submodule)]
pub mod printer {
    use pyo3::{types::PyModule, Bound, PyResult};

    use crate::utils::fix_imports;

    #[pymodule_export]
    use super::Printer;

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        fix_imports(m, "craft_cli._rs.printer")
    }
}
