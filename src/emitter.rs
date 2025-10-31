//! The Emitter class and its associated helpers.

use std::{fs::{self, File}, io::Write as _};

use pyo3::{Bound, PyResult, Python, pyclass, pymethods, types::PyType};

use crate::printer::{Message, MessageType, Printer, Target, Verbosity};

/// Emitter
#[pyclass]
struct Emitter {
    /// Internal printer instance for sending messages.
    ///
    /// Executes I/O operations in a separate thread to make all logging non-blocking.
    printer: Printer,

    /// A handle to the desired log file.
    log: File,

    /// The original filepath of the log file.
    log_filepath: String,

    /// The base URL for error messages.
    docs_base_url: String,

    /// The verbosity mode.
    verbosity: Verbosity,
}

#[pymethods]
impl Emitter {
    /// Construct a new `Emitter` from Python.
    #[new]
    fn new(
        py: Python<'_>,
        log_filepath: String,
        verbosity: Verbosity,
        docs_base_url: &str,
    ) -> PyResult<Self> {
        let mut printer = Printer::new();

        // Spawn the printer thread without using the GIL at all
        // This is necessary to avoid deadlocks when using OnceCell, see the link below
        // for more information.
        // https://pyo3.rs/v0.25.1/faq.html#im-experiencing-deadlocks-using-pyo3-with-stdsynconcelock-stdsynclazylock-lazy_static-and-once_cell
        py.allow_threads(|| printer.start(verbosity));

        let log = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&log_filepath)?;

        Ok(Self {
            printer,
            log,
            log_filepath,
            docs_base_url: docs_base_url.trim_end_matches('/').to_string(),
            verbosity,
        })
    }

    /// Create a log filepath from the app name as an easy default.
    #[classmethod]
    fn log_filepath_from_name(_cls: &Bound<'_, PyType>, app_name: String) -> String {
        let dirs = xdg::BaseDirectories::with_prefix(app_name);
        let mut p = dirs
            .get_data_home()
            .unwrap_or(std::env::current_dir().expect("Could not find suitable log location. As a fallback, make sure the current directory exists."));

        let now = jiff::Timestamp::now();
        let filename = format!("{}.log", now.strftime("%Y%m%d-%H%M%S.%f"));
        p.extend(["log", &filename]);
        p.to_string_lossy().into()
    }

    /// Get the current verbosity mode of the emitter.
    fn get_mode(&self) -> Verbosity {
        self.verbosity
    }

    /// Verbose information.
    ///
    /// Useful for providing more information to the user that isn't particularly
    /// helpful for "regular use"
    fn verbose(&mut self, mut text: String) {
        let timestamped = Self::apply_timestamp(&text);
        self.log.write(timestamped.as_ref())?;
        let target = match self.verbosity {
            Verbosity::Brief | Verbosity::Quiet => Target::Null,
            Verbosity::Verbose => Target::Stderr,
            _ => {
                text = timestamped;
                Target::Stderr
            }
        };

        let message = Message {
            text,
            target,
            model: MessageType::Debug,
        };

        self.printer.send(message);
    }

    /// Debug information.
    ///
    /// Use to record anything that the user may not want to normally see, but
    /// would be useful for the app developers to understand why things may be
    /// failing.
    fn debug(&mut self, mut text: String) {
        let target = match self.verbosity {
            Verbosity::Brief | Verbosity::Quiet | Verbosity::Verbose => Target::Null,
            _ => Target::Stderr,
        };

        Self::apply_timestamp(&mut text);

        let message = Message {
            text,
            target,
            model: MessageType::Debug,
        };

        self.printer.send(message);
    }

    /// Trace information.
    ///
    /// Use to expose system-generated information which in general would be
    /// overwhelming for debugging purposes but sometimes needed for more
    /// in-depth analysis.
    fn trace(&mut self, mut text: String) {
        let timestamped =

        let target = match self.verbosity {
            Verbosity::Trace => Target::Stderr,
            _ => Target::Null,
        };

        Self::apply_timestamp(&mut text);

        let message = Message {
            text,
            target,
            model: MessageType::Trace,
        };

        self.printer.send(message);
    }

    /// Progress information.
    ///
    /// This is normally used to present several related messages relaying how
    /// a task is going. If a progress message is important enough that it
    /// shouldn't be overwritten by the next ones, use "permanent=True".
    ///
    /// These messages will be truncated to the terminal's width and overwritten
    /// by the next line (unless in verbose or trace mode, or set to permanent).
    fn progress(&mut self, mut text: String, mut permanent: Option<bool>) {
        let target = match self.verbosity {
            Verbosity::Quiet => {
                permanent = Some(false);
                Target::Null
            }
            Verbosity::Brief => Target::Stderr,
            Verbosity::Verbose => {
                permanent = Some(true);
                Target::Stderr
            }
            _ => {
                permanent = Some(true);
                Self::apply_timestamp(&mut text);
                Target::Stderr
            }
        };

        let msg_obj = Message {
            text,
            model: if permanent.unwrap_or(false) {
                MessageType::ProgPersistent
            } else {
                MessageType::ProgEphemeral
            },
            target,
        };

        self.printer.send(msg_obj);
    }

    /// Show a simple message to the user.
    ///
    /// Ideally used as the final message in a sequence to show a result, as it
    /// goes to stdout unlike other message types.
    fn message(&mut self, text: String) {
        let target = match self.verbosity {
            Verbosity::Quiet => Target::Null,
            _ => Target::Stdout,
        };

        let message = Message {
            text,
            model: MessageType::Info,
            target,
        };

        self.printer.send(message);
    }

    /// Stop the printing infrastructure and print a final message to see the logs.
    fn finish(&mut self) -> PyResult<()> {
        let message = Message {
            text: format!("Full execution log at '{}'", self.log_filepath),
            model: MessageType::Info,
            target: Target::Stderr,
        };
        self.printer.send(message);
        self.printer.stop()?;
        Ok(())
    }
}

impl Emitter {
    /// Apply the timestamp to a message if necessary.
    fn apply_timestamp(text: &String) -> String {
        format!(
            "{} {}",
            jiff::Timestamp::now().strftime("%Y-%m-%D %H:%M:%s%.3f"),
            text
        )
    }

    fn log(&mut self, text: String) -> PyResult<()> {

    }
}

impl Drop for Emitter {
    fn drop(&mut self) {
        self.printer.stop().expect(
            "An unknown error has occurred! The Emitter was not stopped correctly,\
            so context about the error has been lost. Please report this error.",
        );
    }
}
