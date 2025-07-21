use std::{
    sync::mpsc::{self, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::Duration,
};

use lazy_static::lazy_static;

use pyo3::{pyclass, pymethods, pymodule, sync::GILOnceCell, PyErr, PyResult, Python};

/// Types of message for printing.
#[non_exhaustive]
#[derive(Clone, Copy)]
#[pyclass]
#[expect(non_camel_case_types)]
pub enum MessageType {
    PROG_PERSISTENT,
    PROG_EPHEMERAL,
    WARNING,
    ERROR,
    DEBUG,
    TRACE,
    INFO,
}

/// A single message to be sent, and what type of message it is.
#[derive(Clone)]
#[pyclass]
pub struct Message {
    message: String,
    model: MessageType,
}

#[pymethods]
impl Message {
    #[new]
    pub fn new(message: String, model: MessageType) -> Self {
        Self { message, model }
    }
}

impl Message {
    pub fn determine_stream(&self, mode: &Mode) -> Option<console::TermTarget> {
        use self::{MessageType::*, Mode::*};
        use console::TermTarget::*;
        match self.model {
            PROG_PERSISTENT | PROG_EPHEMERAL => Stdout.into(),
            WARNING => Stderr.into(),
            ERROR => Stderr.into(),
            DEBUG => match mode {
                VERBOSE => Stdout.into(),
                BRIEF => None,
            },
            TRACE => match mode {
                VERBOSE => Stdout.into(),
                BRIEF => None,
            },
            INFO => match mode {
                VERBOSE => Stdout.into(),
                BRIEF => None,
            },
        }
    }
}

/// Verbosity modes.
#[non_exhaustive]
#[derive(Clone)]
#[pyclass]
pub enum Mode {
    BRIEF,
    VERBOSE,
}

/// An internal printer object meant to print from a separate thread.
///
/// Holds an exclusive lock over stdout and stderr and frees it only
/// upon being dropped.
struct InnerPrinter {
    channel: mpsc::Receiver<Message>,
    stdout: console::Term,
    stderr: console::Term,
    mode: Mode,
    needs_overwrite: bool,
}

impl InnerPrinter {
    pub fn new(mode: Mode, channel: mpsc::Receiver<Message>) -> Self {
        let result = Self {
            stdout: console::Term::stdout(),
            stderr: console::Term::stderr(),
            channel,
            mode,
            needs_overwrite: false,
        };

        // Hide the terminal cursor while taking control
        result.stdout.hide_cursor().unwrap();

        result
    }

    /// Begin listening for messages on `self.channel`.
    ///
    /// This method will block execution until the the corresponding `Sender` for
    /// `self.channel` is closed. As such, it is strongly recommended to only invoke
    /// this from a dedicated thread.
    pub fn listen(&mut self) -> PyResult<()> {
        let mut maybe_prv_msg: Option<Message> = None;
        'thread: loop {
            // Wait the standard 3 seconds for a message
            match self.await_message(Duration::from_secs(3)) {
                Ok(msg) => {
                    // Store the most recently received message in case we need to
                    // begin displaying a spin loader
                    maybe_prv_msg = Some(msg.clone());
                    self.handle_message(msg)?
                }
                // Break out of this loop if the channel is closed
                Err(RecvTimeoutError::Disconnected) => break,
                // If the three seconds elapsed, spin
                Err(RecvTimeoutError::Timeout) => {
                    // Don't actually spin if there isn't a previous message to spin on
                    let prv_msg = match maybe_prv_msg {
                        Some(_) => maybe_prv_msg.take().unwrap(),
                        None => continue,
                    };

                    // Get the progress spinner style
                    lazy_static! {
                        static ref STYLE: indicatif::ProgressStyle =
                            indicatif::ProgressStyle::with_template("{spinner} {msg} ({elapsed})")
                                .unwrap();
                    }

                    use console::TermTarget::*;
                    let spinner = prv_msg.determine_stream(&self.mode).map(|target| {
                        let s = match target {
                            Stdout => indicatif::ProgressBar::with_draw_target(
                                None,
                                indicatif::ProgressDrawTarget::stdout(),
                            ),
                            Stderr => indicatif::ProgressBar::with_draw_target(
                                None,
                                indicatif::ProgressDrawTarget::stderr(),
                            ),
                            // This variant is never used
                            ReadWritePair(_) => unreachable!(),
                        }
                        .with_message(prv_msg.message.clone())
                        .with_style(STYLE.clone());

                        self.stdout.clear_last_lines(1).unwrap();
                        s.enable_steady_tick(Duration::from_millis(100));
                        s
                    });

                    // Kick off another loop similar to the one above, but on a more
                    // frequent poll interval.
                    loop {
                        // Same logic as the outer match statement, but with a much more
                        // frequent check.
                        match self.await_message(Duration::from_millis(100)) {
                            Ok(msg) => {
                                if let Some(s) = spinner {
                                    s.finish_and_clear();
                                    self.handle_message(prv_msg)?;
                                }
                                self.handle_message(msg)?;
                                break;
                            }
                            Err(RecvTimeoutError::Disconnected) => break 'thread,
                            Err(RecvTimeoutError::Timeout) => continue,
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Helper method for receiving a message from `self.channel`
    fn await_message(
        &mut self,
        timeout: Duration,
    ) -> ::std::result::Result<Message, RecvTimeoutError> {
        self.channel.recv_timeout(timeout)
    }

    /// Routing method for sending a message to the proper printing logic for a given
    /// message type.
    fn handle_message(&mut self, msg: Message) -> PyResult<()> {
        use self::MessageType::*;
        match msg.model {
            INFO => self.print(msg),
            ERROR => self.error(msg),
            PROG_EPHEMERAL => self.progress(msg, false),
            PROG_PERSISTENT => self.progress(msg, true),
            _ => unimplemented!(),
        }
    }

    fn handle_overwrite(&mut self) -> PyResult<()> {
        if self.needs_overwrite {
            self.stdout.clear_last_lines(1)?;
        }
        Ok(())
    }

    /// Print a simple message to stdout.
    fn print(&mut self, message: Message) -> PyResult<()> {
        self.stdout.write_line(&message.message)?;
        Ok(())
    }

    /// Print a simple message to stderr.
    fn error(&mut self, message: Message) -> PyResult<()> {
        self.handle_overwrite()?;
        self.stderr.write_line(&message.message)?;
        Ok(())
    }

    /// Print progress on a task.
    pub fn progress(&mut self, message: Message, permanent: bool) -> PyResult<()> {
        self.handle_overwrite()?;
        self.needs_overwrite = match self.mode {
            Mode::BRIEF => !permanent,
            Mode::VERBOSE => false,
        };
        self.print(message)?;
        Ok(())
    }
}

impl Drop for InnerPrinter {
    /// Restore the cursor when releasing control of the terminal.
    fn drop(&mut self) {
        self.stdout.show_cursor().unwrap();
    }
}

/// Public API for printing. Stores a handle to the thread that `InnerPrinter` is
/// printing from, and a channel to send messages.
#[derive(Default)]
#[pyclass]
pub struct Printer {
    handle: GILOnceCell<JoinHandle<PyResult<()>>>,
    channel: GILOnceCell<mpsc::Sender<Message>>,
}

#[pymethods]
impl Printer {
    #[new]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a thread to begin listening for messages to print.
    #[pyo3(signature = (mode))]
    pub fn start(&mut self, py: Python<'_>, mode: Mode) {
        let (send, recv) = mpsc::channel();

        if let Err(_) = self.channel.set(py, send) {
            panic!("Printer was already started!");
        }

        let handle = thread::spawn(move || -> PyResult<()> {
            let mut printer = InnerPrinter::new(mode, recv);
            printer.listen()?;
            Ok(())
        });

        self.handle.set(py, handle).unwrap();
    }

    pub fn stop(&mut self) -> PyResult<()> {
        // Dropping the channel closes it, which will be seen by the other thread as a
        // stopping condition
        _ = self.channel.take();
        if let Some(handle) = self.handle.take() {
            if let Err(e) = handle.join() {
                // PyErr is guaranteed as the return type, so we can blindly
                // downcast
                return Err(*e.downcast::<PyErr>().unwrap());
            }
        }

        Ok(())
    }

    /// Send a message to the InnerPrinter for displaying
    #[pyo3(signature = (msg))]
    pub fn send(&self, py: Python<'_>, msg: Message) -> PyResult<()> {
        match self.channel.get(py) {
            Some(chan) => chan.send(msg).unwrap(),
            None => panic!("Receiver closed early?"),
        }

        Ok(())
    }
}

#[pymodule(submodule)]
#[pyo3(module = "craft_cli._rs.printer")]
pub mod printer {
    use pyo3::{types::PyModule, Bound, PyResult};

    use crate::utils::fix_imports;

    #[pymodule_export]
    use super::{Message, MessageType, Mode, Printer};

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        fix_imports(m, "craft_cli._rs.printer")
    }
}
