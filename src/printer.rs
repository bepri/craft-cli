//! The `Printer` module for handling messages to a terminal.

use std::{
    sync::{
        LazyLock, OnceLock,
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use pyo3::{PyErr, PyResult, pyclass};

use console::TermTarget;

use crate::utils::log;

/// Representation of which stream should be targeted by a message.
#[derive(Debug, Clone)]
pub enum Target {
    /// Target the stdout stream.
    Stdout,

    /// Target the stderr stream.
    Stderr,

    /// Target no stream at all.
    Null,
}

/// Types of message for printing.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
#[pyclass]
pub enum MessageType {
    /// A persistent progress message that will remain on the console.
    ///
    /// For a non-permanent message, see `ProgEphemeral`.
    ProgPersistent,

    /// An ephemeral progress message that will be overwritten by the next message.
    ///
    /// For a permanent message, see `ProgPersistent`.
    ProgEphemeral,

    /// A warning message.
    Warning,

    /// An error message.
    Error,

    /// A debugging info message.
    Debug,

    /// A trace info message.
    Trace,

    /// An informational message.
    Info,
}

/// A single message to be sent, and what type of message it is.
#[derive(Clone, Debug)]
pub struct Message {
    /// The message to be printed.
    pub(crate) text: String,

    /// The type of message to send.
    pub(crate) model: MessageType,

    /// Where the message should be sent.
    pub(crate) target: Target,
}

impl Message {
    /// Calculate which stream a message should go to based on its model.
    pub fn determine_stream(&self, mode: Verbosity) -> Option<TermTarget> {
        use self::Verbosity::*;
        use TermTarget::*;
        match self.model {
            MessageType::ProgPersistent | MessageType::ProgEphemeral => Stdout.into(),
            MessageType::Warning | MessageType::Error => Stderr.into(),
            MessageType::Debug | MessageType::Trace | MessageType::Info => match mode {
                Verbose => Stdout.into(),
                _ => None,
            },
        }
    }
}

/// Verbosity modes.
#[non_exhaustive]
#[derive(Clone, Copy)]
#[pyclass]
pub enum Verbosity {
    /// Quiet output. Most messages should not be output at all.
    Quiet,

    /// Brief output. Most messages should be ephemeral and all debugging-style message
    /// models should be skipped.
    Brief,

    /// Verbose mode. All messages should be persistent and all debugging-style messages
    /// kept.
    Verbose,

    /// Trace mode. The absolute maximum amount of information should be printed.
    Trace,
}

/// An internal printer object meant to print from a separate thread.
struct InnerPrinter {
    /// A channel upon which messages can be read.
    ///
    /// If this channel is found to be closed, the program is over and this struct
    /// should begin to destruct itself.
    channel: mpsc::Receiver<Message>,

    /// A handle on stdout.
    stdout: console::Term,

    /// A handle on stderr.
    stderr: console::Term,

    /// Printing verbosity mode.
    mode: Verbosity,

    /// A flag indicating if the previous line should be overwritten when printing
    /// the next.
    needs_overwrite: bool,
}

impl InnerPrinter {
    /// Instantiate a new `InnerPrinter`.
    pub fn new(mode: Verbosity, channel: mpsc::Receiver<Message>) -> Self {
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
        static MAIN_STYLE: LazyLock<indicatif::ProgressStyle> = LazyLock::new(|| {
            indicatif::ProgressStyle::with_template("{spinner} {msg} ({elapsed})").unwrap()
        });
        let mut spinner: Option<indicatif::ProgressBar> = None;

        let mut maybe_prv_msg: Option<Message> = None;

        loop {
            // Wait the standard 3 seconds for a message
            match self.await_message(Duration::from_secs(3)) {
                Ok(msg) => {
                    // If we were spinning, stop
                    if let Some(s) = spinner.take()
                        && let Some(mut prv_msg) = maybe_prv_msg.take()
                    {
                        s.finish_and_clear();
                        self.needs_overwrite = false;
                        let dur = indicatif::HumanDuration(s.elapsed());
                        prv_msg.text = format!("{} (took {:#})", prv_msg.text, dur);
                        self.handle_message(&prv_msg)?;
                        log(format!("clearing {}", prv_msg.text));
                    }
                    // Store the most recently received message in case we need to
                    // begin displaying a spin loader
                    maybe_prv_msg = Some(msg.clone());
                    self.handle_message(&msg)?;
                }
                // Break out of this loop if the channel is closed
                Err(RecvTimeoutError::Disconnected) => break,
                // If the three seconds elapsed, spin
                Err(RecvTimeoutError::Timeout) => {
                    // If we're already spinning on a message, keep waiting
                    if spinner.is_some() {
                        continue;
                    }
                    // If there's a previous message to spin on, then,
                    spinner = maybe_prv_msg.as_ref().and_then(|prv_msg| {
                        // If there is a stream to print to,
                        prv_msg.determine_stream(self.mode).map(|target| {
                            // Construct a spinner
                            let s = match target {
                                TermTarget::Stdout => indicatif::ProgressBar::with_draw_target(
                                    None,
                                    indicatif::ProgressDrawTarget::stdout(),
                                ),
                                TermTarget::Stderr => indicatif::ProgressBar::with_draw_target(
                                    None,
                                    indicatif::ProgressDrawTarget::stderr(),
                                ),
                                // This variant is never used
                                TermTarget::ReadWritePair(_) => unreachable!(),
                            }
                            .with_message(prv_msg.text.clone())
                            .with_style(MAIN_STYLE.clone())
                            .with_elapsed(Duration::from_secs(3));

                            // It doesn't matter which stream we clear, the line we're about to
                            // spin is wiped either way
                            self.stdout.clear_last_lines(1).unwrap();
                            // Start spinning
                            s.enable_steady_tick(Duration::from_millis(100));
                            log(format!("spinning on {}", prv_msg.text));
                            s
                        })
                    });
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
    fn handle_message(&mut self, msg: &Message) -> PyResult<()> {
        use self::MessageType::*;
        if let Target::Null = msg.target {
            return Ok(());
        }
        log(format!("writing {msg:?}"));
        match msg.model {
            Info => self.print(msg),
            Error => self.error(msg),
            ProgEphemeral => self.progress(msg, false),
            ProgPersistent => self.progress(msg, true),
            _ => unimplemented!(),
        }
    }

    /// Handle the need (or lackthereof) to overwrite the previous line.
    fn handle_overwrite(&mut self) -> PyResult<()> {
        if self.needs_overwrite {
            log("overwriting!");
            self.stdout.clear_last_lines(1)?;
        }
        Ok(())
    }

    /// Print a simple message to stdout.
    fn print(&mut self, message: &Message) -> PyResult<()> {
        self.stdout.write_line(&message.text)?;
        Ok(())
    }

    /// Print a simple message to stderr.
    fn error(&mut self, message: &Message) -> PyResult<()> {
        self.handle_overwrite()?;
        self.stderr.write_line(&message.text)?;
        Ok(())
    }

    /// Print progress on a task.
    pub fn progress(&mut self, message: &Message, permanent: bool) -> PyResult<()> {
        self.handle_overwrite()?;
        self.needs_overwrite = !permanent;
        self.print(message)?;
        Ok(())
    }
}

impl Drop for InnerPrinter {
    /// Restore the cursor when releasing control of the terminal.
    fn drop(&mut self) {
        self.handle_overwrite().unwrap();
        self.stdout.show_cursor().unwrap();
    }
}

/// Public API for printing. Stores a handle to the thread that `InnerPrinter` is
/// printing from, and a channel to send messages.
#[derive(Default)]
pub struct Printer {
    /// A handle on the thread running the `InnerPrinter` instance.
    handle: OnceLock<JoinHandle<PyResult<()>>>,

    /// A channel to send messages to the `InnerPrinter` instance.
    channel: OnceLock<mpsc::Sender<Message>>,
}

impl Printer {
    /// The `__init__` Python method to create a printer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a thread to begin listening for messages to print.
    pub fn start(&mut self, mode: Verbosity) {
        let (send, recv) = mpsc::channel();

        assert!(
            self.channel.set(send).is_ok(),
            "Printer was already started!"
        );

        let handle = thread::spawn(move || -> PyResult<()> {
            let mut printer = InnerPrinter::new(mode, recv);
            printer.listen()?;
            Ok(())
        });

        self.handle.set(handle).unwrap();
    }

    /// Stop printing.
    ///
    /// This ends the `InnerPrinter` instance's thread.
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

    /// Send a message to the `InnerPrinter` for displaying
    pub fn send(&self, msg: Message) {
        match self.channel.get() {
            Some(chan) => chan.send(msg).unwrap(),
            None => panic!("Receiver closed early?"),
        }
    }
}

impl Drop for Printer {
    fn drop(&mut self) {
        self.stop().expect("An error was encountered while logging. Tear down the printer properly to view the error.");
    }
}
