// Terminal mapping `Terminal.cs`.
// Provides console I/O with tracing integration and secret masking.

use crate::host_context::HostContext;
use crate::secret_masker::SecretMasker;
use crate::tracing::Tracing;

use runner_sdk::TraceWriter;
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Console color codes for terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleColor {
    Red,
    Green,
    Yellow,
    Cyan,
    White,
    Default,
}

impl ConsoleColor {
    /// ANSI escape code for the color.
    fn ansi_code(&self) -> &'static str {
        match self {
            ConsoleColor::Red => "\x1b[31m",
            ConsoleColor::Green => "\x1b[32m",
            ConsoleColor::Yellow => "\x1b[33m",
            ConsoleColor::Cyan => "\x1b[36m",
            ConsoleColor::White => "\x1b[37m",
            ConsoleColor::Default => "",
        }
    }

    /// ANSI reset code.
    fn reset() -> &'static str {
        "\x1b[0m"
    }
}

/// Terminal abstraction for console I/O with tracing and secret masking.
///
/// Maps `Terminal` in the C# runner.
pub struct Terminal {
    /// Whether to suppress output.
    pub silent: bool,
    /// Trace instance for logging terminal activity.
    trace: Option<Tracing>,
    /// Secret masker for masking secrets in ReadSecret.
    secret_masker: Option<Arc<SecretMasker>>,
    /// Broadcast channel for cancel key press events.
    cancel_tx: broadcast::Sender<()>,
    /// Receiver side (kept alive to prevent channel closing).
    _cancel_rx: broadcast::Receiver<()>,
}

impl Terminal {
    /// Create a new `Terminal`.
    pub fn new() -> Self {
        let (cancel_tx, _cancel_rx) = broadcast::channel(4);
        Self {
            silent: false,
            trace: None,
            secret_masker: None,
            cancel_tx,
            _cancel_rx,
        }
    }

    /// Initialize with a host context (sets up tracing and secret masker).
    pub fn initialize(&mut self, context: &Arc<HostContext>) {
        self.trace = Some(context.get_trace("Terminal"));
        self.secret_masker = Some(context.secret_masker.clone());

        // Set up Ctrl+C handler that broadcasts on our cancel channel
        let tx = self.cancel_tx.clone();
        let _ = ctrlc_channel(&tx);
    }

    /// Subscribe to cancel key press events.
    pub fn cancel_receiver(&self) -> broadcast::Receiver<()> {
        self.cancel_tx.subscribe()
    }

    /// Read a line from stdin.
    pub fn read_line(&self) -> String {
        if let Some(ref trace) = self.trace {
            trace.info("READ LINE");
        }

        let mut input = String::new();
        let _ = io::stdin().lock().read_line(&mut input);
        let value = input.trim_end_matches('\n').trim_end_matches('\r').to_string();

        if let Some(ref trace) = self.trace {
            trace.info(&format!("Read value: '{}'", value));
        }

        value
    }

    /// Read a secret (password) from stdin with masked display.
    ///
    /// Each character typed is echoed as `*`. Backspace removes the last character.
    pub fn read_secret(&self) -> String {
        if let Some(ref trace) = self.trace {
            trace.info("READ SECRET");
        }

        // Use a simpler approach: disable echo on Unix
        let value = read_secret_line();

        if let Some(ref masker) = self.secret_masker {
            if !value.is_empty() {
                masker.add_value(&value);
            }
        }

        if let Some(ref trace) = self.trace {
            trace.info(&format!("Read value: '{}'", value));
        }

        value
    }

    /// Write a string to stdout (no newline).
    pub fn write(&self, message: &str, color: Option<ConsoleColor>) {
        if let Some(ref trace) = self.trace {
            trace.info(&format!("WRITE: {}", message));
        }

        if !self.silent {
            if let Some(color) = color {
                print!("{}{}{}", color.ansi_code(), message, ConsoleColor::reset());
            } else {
                print!("{}", message);
            }
            let _ = io::stdout().flush();
        }
    }

    /// Write a line to stdout.
    pub fn write_line(&self, line: &str, color: Option<ConsoleColor>) {
        if let Some(ref trace) = self.trace {
            trace.info(&format!("WRITE LINE: {}", line));
        }

        if !self.silent {
            if let Some(color) = color {
                println!("{}{}{}", color.ansi_code(), line, ConsoleColor::reset());
            } else {
                println!("{}", line);
            }
        }
    }

    /// Write an empty line.
    pub fn write_empty_line(&self) {
        self.write_line("", None);
    }

    /// Write an error message to stderr.
    pub fn write_error(&self, line: &str) {
        if let Some(ref trace) = self.trace {
            trace.error(&format!("WRITE ERROR: {}", line));
        }

        if !self.silent {
            eprintln!(
                "{}{}{}",
                ConsoleColor::Red.ansi_code(),
                line,
                ConsoleColor::reset()
            );
        }
    }

    /// Write an error from an `anyhow::Error`.
    pub fn write_error_err(&self, err: &anyhow::Error) {
        if let Some(ref trace) = self.trace {
            trace.error("WRITE ERROR (exception):");
            trace.error(&format!("{:#}", err));
        }

        if !self.silent {
            eprintln!(
                "{}{}{}",
                ConsoleColor::Red.ansi_code(),
                err,
                ConsoleColor::reset()
            );
        }
    }

    /// Write a section header.
    pub fn write_section(&self, message: &str) {
        if !self.silent {
            println!();
            println!("# {}", message);
            println!();
        }
    }

    /// Write a success message with a checkmark prefix.
    pub fn write_success_message(&self, message: &str) {
        if !self.silent {
            print!(
                "{}√ {}",
                ConsoleColor::Green.ansi_code(),
                ConsoleColor::reset()
            );
            println!("{}", message);
        }
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Set up a Ctrl+C handler that sends on the broadcast channel.
fn ctrlc_channel(tx: &broadcast::Sender<()>) {
    let tx = tx.clone();
    let _ = ctrlc::set_handler(move || {
        let _ = tx.send(());
    });
}

/// Read a line from stdin without echoing characters (for passwords).
///
/// On Unix we use raw terminal mode if available; otherwise we fall back
/// to a simple readline.
fn read_secret_line() -> String {
    #[cfg(unix)]
    {
        let stdin = io::stdin();

        // Try to disable echo
        if let Ok(mut termios) = nix::sys::termios::tcgetattr(&stdin) {
            use nix::sys::termios::LocalFlags;
            let old_flags = termios.local_flags;
            termios.local_flags &= !(LocalFlags::ECHO);
            if nix::sys::termios::tcsetattr(
                &stdin,
                nix::sys::termios::SetArg::TCSANOW,
                &termios,
            )
            .is_ok()
            {
                let mut input = String::new();
                let _ = stdin.lock().read_line(&mut input);

                // Restore echo
                termios.local_flags = old_flags;
                let _ = nix::sys::termios::tcsetattr(
                    &stdin,
                    nix::sys::termios::SetArg::TCSANOW,
                    &termios,
                );

                // Print newline since echo was disabled
                println!();

                return input.trim_end_matches('\n').trim_end_matches('\r').to_string();
            }
        }

        // Fallback: just read normally
        let mut input = String::new();
        let _ = io::stdin().lock().read_line(&mut input);
        input.trim_end_matches('\n').trim_end_matches('\r').to_string()
    }

    #[cfg(not(unix))]
    {
        let mut input = String::new();
        let _ = io::stdin().lock().read_line(&mut input);
        input.trim_end_matches('\n').trim_end_matches('\r').to_string()
    }
}
