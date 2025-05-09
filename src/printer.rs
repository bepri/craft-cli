//! The output (for different destinations) handler and helper functions.

use std::{
    fmt::Display,
    fs::File,
    hint,
    io::{IsTerminal, Write},
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, OnceLock, RwLock, RwLockReadGuard,
    },
    thread::{self, JoinHandle, Thread},
    time::Duration,
};

use jiff::Timestamp;
use lazy_static::lazy_static;
use pyo3::{
    exceptions::{PyOSError, PyRuntimeError},
    pyclass, pymethods, pymodule,
    types::PyAnyMethods,
    FromPyObject, PyResult,
};

use crate::utils::get_substring;

const SPINNER_THRESHHOLD: f32 = 0.0;
const SPINNER_DELAY: f32 = 0.001;

const ANSI_CLEAR_LINE_TO_END: &str = "\u{001b}[K";
const ANSI_HIDE_CURSOR: &str = "\u{001b}[?25l";
const ANSI_SHOW_CURSOR: &str = "\u{001b}[?25h";

const EXPECT_NO_POISON: &str =
    "Only fails if a panic occurs in another thread, which would be irrecoverable anyways.";

struct Spinner {
    sender: Sender<Option<MessageInfo>>,
    receiver: Mutex<Receiver<Option<MessageInfo>>>,
    lock: Mutex<()>,
    under_supervision: Option<MessageInfo>,
    thread_handle: Option<JoinHandle<()>>,
}

impl Spinner {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver: Mutex::new(receiver),
            lock: Mutex::new(()),
            under_supervision: None,
            thread_handle: None,
        }
    }

    pub fn run<'sf, SF: FnMut(&mut MessageInfo, &str) -> PyResult<()> + Send>(
        &mut self,
        mut spin_func: SF,
    ) -> PyResult<()> {
        // thread::spawn(|| self.thread_closure(&'sf mut spin_func));
        Ok(())
    }

    fn thread_closure<SF: FnMut(&mut MessageInfo, &str) -> PyResult<()> + Send>(
        &mut self,
        mut spin_func: SF,
    ) -> PyResult<()> {
        let mut prv_msg: Option<MessageInfo> = None;
        const SPINCHARS: [char; 4] = ['-', '\\', '|', '/'];
        let receiver = self.receiver.get_mut().expect(EXPECT_NO_POISON);

        loop {
            let t_init = Timestamp::now();
            let new_msg = match receiver.recv_timeout(Duration::from_secs_f32(SPINNER_THRESHHOLD)) {
                Err(_) => {
                    return Err(PyRuntimeError::new_err(
                        "Internal error: spin queue sender was closed early",
                    ));
                }
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    // We waited too much, start to show a spinner (if we have a message to spin)
                    // until we have more info
                    if prv_msg.as_ref().is_none_or(|msg| msg.end_line) {
                        continue;
                    }

                    let mut spin_it = SPINCHARS.iter().cycle();

                    // Open a new scope to hold the lock
                    {
                        let _lock = self.lock.lock().expect(EXPECT_NO_POISON);

                        loop {
                            let t_delta = Timestamp::now() - t_init;
                            let spintext = format!(
                                " {} {:.1}",
                                spin_it
                                    .next()
                                    .expect("Cyclic iterator - will always yield more."),
                                t_delta
                            );
                            spin_func(
                                prv_msg.as_mut().expect("Checked earlier in the function"),
                                &spintext,
                            )?;

                            match receiver.recv_timeout(Duration::from_secs_f32(SPINNER_DELAY)) {
                                Err(_) => {
                                    return Err(PyRuntimeError::new_err(
                                        "Internal error: spin queue sender was closed early",
                                    ));
                                }
                                Ok(Some(msg)) => break msg,
                                Ok(None) => continue,
                            }
                        }
                    }
                }
            };

            prv_msg = Some(new_msg);
            if prv_msg.is_none() {
                break;
            }
        }

        Ok(())
    }

    /// Supervise a message to spin it if it remains too long.
    fn supervise(&mut self, message: MessageInfo) {
        if Some(&message) == self.under_supervision.as_ref() {
            return;
        }

        self.under_supervision = Some(message.clone());
        self.sender.send(Some(message));
        self.lock.lock().expect(EXPECT_NO_POISON);
    }

    pub fn stop(&mut self) {
        self.sender.send(None);
        if let Some(handle) = self.thread_handle.take() {
            handle.join();
        }
    }
}

#[repr(transparent)]
#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
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

    pub fn write<AnyStr>(&mut self, message: AnyStr) -> PyResult<()>
    where
        AnyStr: AsRef<[u8]>,
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

    pub fn is_tty(&self) -> bool {
        match self.0 {
            Some(ref stream_lock) => {
                let stream = stream_lock.lock().expect(EXPECT_NO_POISON);
                stream.is_terminal()
            }
            None => false,
        }
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

#[repr(transparent)]
#[derive(Clone, Default)]
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

impl<AnyStr> PartialEq<AnyStr> for TermPrefix
where
    AnyStr: AsRef<[u8]>,
{
    fn eq(&self, other: &AnyStr) -> bool {
        AsRef::<[u8]>::as_ref(&self.read().as_str()) == other.as_ref()
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
    spin_lock: Arc<Mutex<()>>,
    spin_thread_handle: OnceLock<JoinHandle<PyResult<()>>>,
    spin_thread_sender: Option<Sender<Option<MessageInfo>>>,
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
            log,
            stopped: Default::default(),
            prv_msg: Default::default(),
            terminal_prefix: Default::default(),
            secrets: Default::default(),
            spin_lock: Default::default(),
            spin_thread_handle: Default::default(),
            spin_thread_sender: Default::default(),
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
    fn write_line_terminal(&mut self, message: &mut MessageInfo, spintext: &str) -> PyResult<()> {
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

    fn spin(&mut self, message: &mut MessageInfo, spintext: &str) -> PyResult<()> {
        if message.stream.is_tty() {
            self.write_line_terminal(message, spintext)?;
        }

        Ok(())
    }

    fn supervise(&mut self, shared_self: Arc<Mutex<Self>>, message: Option<MessageInfo>) {
        self.spin_thread_handle.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            let lock = self.spin_lock.clone();
            self.spin_thread_sender = Some(sender);
            thread::spawn(|| Self::thread_closure(shared_self, receiver, lock))
        });

        self.spin_thread_sender
            .as_mut()
            .expect("Always initialized above")
            .send(message);
    }

    fn thread_closure(
        printer: Arc<Mutex<Self>>,
        receiver: Receiver<Option<MessageInfo>>,
        spin_lock: Arc<Mutex<()>>,
    ) -> PyResult<()> {
        let mut prv_msg: Option<MessageInfo> = None;
        const SPINCHARS: [char; 4] = ['-', '\\', '|', '/'];
        let mut printer = printer.lock().expect(EXPECT_NO_POISON);

        loop {
            let t_init = Timestamp::now();
            let new_msg = match receiver.recv_timeout(Duration::from_secs_f32(SPINNER_THRESHHOLD)) {
                Err(_) => {
                    return Err(PyRuntimeError::new_err(
                        "Internal error: spin queue sender was closed early",
                    ));
                }
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    // We waited too much, start to show a spinner (if we have a message to spin)
                    // until we have more info
                    if prv_msg.as_ref().is_none_or(|msg| msg.end_line) {
                        continue;
                    }

                    let mut spin_it = SPINCHARS.iter().cycle();

                    // Open a new scope to hold the lock
                    {
                        let _lock = spin_lock.lock().expect(EXPECT_NO_POISON);

                        loop {
                            let t_delta = Timestamp::now() - t_init;
                            let spintext = format!(
                                " {} {:.1}",
                                spin_it
                                    .next()
                                    .expect("Cyclic iterator - will always yield more."),
                                t_delta
                            );
                            printer.spin(
                                prv_msg.as_mut().expect("Checked earlier in the function"),
                                &spintext,
                            )?;

                            match receiver.recv_timeout(Duration::from_secs_f32(SPINNER_DELAY)) {
                                Err(_) => {
                                    return Err(PyRuntimeError::new_err(
                                        "Internal error: spin queue sender was closed early",
                                    ));
                                }
                                Ok(Some(msg)) => break msg,
                                Ok(None) => continue,
                            }
                        }
                    }
                }
            };

            prv_msg = Some(new_msg);
            if prv_msg.is_none() {
                break;
            }
        }

        Ok(())
    }

    fn get_terminal_width() -> usize {
        termsize::get().map(|sizes| sizes.cols).unwrap_or(80).into()
    }

    /// Format a line to print to the terminal.
    fn format_term_line(
        previous_line_end: &str,
        mut text: String,
        spintext: &str,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;

    mod file_handle {
        use std::{fs, path::Path};

        use tempdir::TempDir;

        use super::*;

        fn get_temp_dir() -> PathBuf {
            TempDir::new("test_file_handle")
                .expect("Could not create temporary directory")
                .into_path()
        }

        fn build_file(path: &Path) -> FileHandle {
            let file = File::options()
                .create_new(true)
                .write(true)
                .open(path)
                .expect("Could not create temporary file");

            FileHandle(Some(Arc::new(Mutex::new(file))))
        }

        #[test]
        fn eq() {
            let temp_dir = get_temp_dir();
            let lhs = build_file(&temp_dir.join("file1.txt"));
            let rhs = lhs.clone();

            assert_eq!(lhs, rhs);
        }

        #[test]
        fn ne() {
            let temp_dir = get_temp_dir();
            let lhs = build_file(&temp_dir.join("file1.txt"));
            let rhs = build_file(&temp_dir.join("file2.txt"));

            assert_ne!(lhs, rhs);
        }

        #[test]
        fn write() {
            let file_path = get_temp_dir().join("file.txt");
            let mut file_handle = build_file(&file_path);

            file_handle.write("🦀").unwrap();

            let file_contents = fs::read(file_path).unwrap();

            assert_eq!(file_contents, "🦀".as_bytes())
        }
    }
}
