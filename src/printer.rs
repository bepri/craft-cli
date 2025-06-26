use std::{
    io::{self, StderrLock, StdoutLock, Write},
    sync::mpsc::{self, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use pyo3::{pyclass, pymethods, pymodule, PyErr, PyResult};

const ANSI_CLEAR_LINE_TO_END: &str = "\u{001b}[K";
const ANSI_HIDE_CURSOR: &str = "\u{001b}[?25l";
const ANSI_SHOW_CURSOR: &str = "\u{001b}[?25h";

/// Types of message for printing.
#[non_exhaustive]
#[derive(Clone, Copy)]
#[pyclass]
#[allow(non_camel_case_types)]
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
struct InnerPrinter<'locks> {
    channel: mpsc::Receiver<Message>,
    stdout: StdoutLock<'locks>,
    stderr: StderrLock<'locks>,
    mode: Mode,
}

impl<'locks> InnerPrinter<'locks> {
    pub fn new(mode: Mode, channel: mpsc::Receiver<Message>) -> Self {
        let mut result = Self {
            stdout: io::stdout().lock(),
            stderr: io::stderr().lock(),
            channel,
            mode,
        };

        // Hide the terminal cursor while taking control
        result.print(ANSI_HIDE_CURSOR.into()).unwrap();

        result
    }

    /// Begin listening for messages on `self.channel`.
    ///
    /// This method will block execution until the the corresponding `Sender` for
    /// `self.channel` is closed. As such, it is strongly recommended to only invoke
    /// this from a dedicated thread.
    pub fn listen(&mut self) -> PyResult<()> {
        let mut maybe_prv_msg: Option<Message> = None;
        let mut time = Instant::now();
        'thread: loop {
            // Wait the standard 3 seconds for a message
            match self.await_message(Duration::from_secs(3)) {
                Ok(msg) => {
                    // Store the most recently received message in case we need to
                    // begin displaying a spin loader
                    time = Instant::now();
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

                    let mut spin_chars = ["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈"].iter().cycle();

                    // Kick off another loop similar to the one above, but on a more
                    // frequent poll interval.
                    loop {
                        // Print a copy of the previous message with a spinner and the
                        // elapsed time
                        let spin_message = format!(
                            "{} {:.1} {}",
                            prv_msg.message,
                            (Instant::now() - time).as_secs_f32(),
                            spin_chars.next().unwrap()
                        );
                        self.handle_message(Message::new(spin_message, prv_msg.model))?;

                        // Same logic as the outer match statement, but with a much more
                        // frequent check.
                        match self.await_message(Duration::from_millis(100)) {
                            Ok(msg) => {
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
            INFO => self.print(msg.message),
            ERROR => self.error(msg.message),
            PROG_EPHEMERAL | PROG_PERSISTENT => {
                self.progress(msg.message, matches!(msg.model, PROG_PERSISTENT))
            }
            _ => unimplemented!(),
        }
    }

    /// Print a simple message to stdout.
    fn print(&mut self, message: String) -> PyResult<()> {
        self.stdout.write(message.as_bytes())?;
        self.stdout.flush()?;
        Ok(())
    }

    /// Print a simple message to stderr.
    fn error(&mut self, message: String) -> PyResult<()> {
        self.stderr.write(message.as_bytes())?;
        self.stderr.flush()?;
        Ok(())
    }

    /// Print progress on a task.
    pub fn progress(&mut self, message: String, permanent: bool) -> PyResult<()> {
        use self::Mode::*;

        let ret_char = match self.mode {
            BRIEF => match permanent {
                true => "\n",
                false => "\r",
            },
            VERBOSE => "\n",
        };

        let prepared_msg = format!(
            "\r{}{}{}",
            ANSI_CLEAR_LINE_TO_END,
            message.trim_end(),
            ret_char
        );

        self.print(prepared_msg)?;

        Ok(())
    }
}

impl<'locks> Drop for InnerPrinter<'locks> {
    /// Restore the cursor when releasing control of the terminal.
    fn drop(&mut self) {
        self.print(ANSI_SHOW_CURSOR.into()).unwrap();
    }
}

/// Public API for printing. Stores a handle to the thread that `InnerPrinter` is
/// printing from, and a channel to send messages.
#[pyclass]
pub struct Printer {
    handle: Option<JoinHandle<PyResult<()>>>,
    channel: Option<mpsc::Sender<Message>>,
}

#[pymethods]
impl Printer {
    #[new]
    pub fn new() -> Self {
        Self {
            handle: None,
            channel: None,
        }
    }

    /// Spawn a thread to begin listening for messages to print.
    pub fn start(&mut self, mode: Mode) {
        if self.handle.is_some() {
            panic!("Printer was already started!");
        }

        let (send, recv) = mpsc::channel();

        let handle = thread::spawn(move || -> PyResult<()> {
            let mut printer = InnerPrinter::new(mode, recv);
            printer.listen()?;
            Ok(())
        });

        self.handle = Some(handle);
        self.channel = Some(send);
    }

    pub fn stop(&mut self) -> PyResult<()> {
        // Dropping the channel closes it, which will be seen by the other thread as a
        // stopping condition
        _ = self.channel.take();
        if let Some(handle) = self.handle.take() {
            if let Err(e) = handle.join() {
                // PyErr is statically guaranteed as the return type, so we can blindly
                // downcast
                return Err(*e.downcast::<PyErr>().unwrap());
            }
        }

        Ok(())
    }

    /// Send a message to the InnerPrinter for displaying
    pub fn send(&self, msg: Message) -> PyResult<()> {
        match self.channel.as_ref() {
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
