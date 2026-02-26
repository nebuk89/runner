use crate::trace::TraceWriter;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The duration to wait after sending SIGINT before escalating to SIGTERM.
const SIGINT_TIMEOUT: Duration = Duration::from_millis(7500);
/// The duration to wait after sending SIGTERM before escalating to SIGKILL.
const SIGTERM_TIMEOUT: Duration = Duration::from_millis(2500);

/// Error type for non-zero process exit codes.
#[derive(Debug, thiserror::Error)]
#[error(
    "Exit code {exit_code} returned from process: file name '{file_name}', arguments '{arguments}'."
)]
pub struct ProcessExitCodeError {
    pub exit_code: i32,
    pub file_name: String,
    pub arguments: String,
}

/// Event data for a line received from stdout or stderr.
#[derive(Debug, Clone)]
pub struct ProcessDataReceivedEventArgs {
    pub data: String,
}

/// A process lifecycle manager that spawns a child process, reads stdout/stderr
/// on separate tasks, supports graceful cancellation (SIGINT → SIGTERM → SIGKILL),
/// and delivers output lines through channels.
///
/// Maps `ProcessInvoker.cs` from the C# SDK.
pub struct ProcessInvoker {
    trace: Arc<dyn TraceWriter>,
    /// Channel for stdout lines. Subscribe via `take_stdout_receiver`.
    stdout_tx: mpsc::UnboundedSender<ProcessDataReceivedEventArgs>,
    stdout_rx: Option<mpsc::UnboundedReceiver<ProcessDataReceivedEventArgs>>,
    /// Channel for stderr lines. Subscribe via `take_stderr_receiver`.
    stderr_tx: mpsc::UnboundedSender<ProcessDataReceivedEventArgs>,
    stderr_rx: Option<mpsc::UnboundedReceiver<ProcessDataReceivedEventArgs>>,
}

impl ProcessInvoker {
    /// Create a new `ProcessInvoker` with the given trace writer.
    pub fn new(trace: Arc<dyn TraceWriter>) -> Self {
        let (stdout_tx, stdout_rx) = mpsc::unbounded_channel();
        let (stderr_tx, stderr_rx) = mpsc::unbounded_channel();
        Self {
            trace,
            stdout_tx,
            stdout_rx: Some(stdout_rx),
            stderr_tx,
            stderr_rx: Some(stderr_rx),
        }
    }

    /// Take the stdout receiver. Can only be called once; subsequent calls return `None`.
    pub fn take_stdout_receiver(
        &mut self,
    ) -> Option<mpsc::UnboundedReceiver<ProcessDataReceivedEventArgs>> {
        self.stdout_rx.take()
    }

    /// Take the stderr receiver. Can only be called once; subsequent calls return `None`.
    pub fn take_stderr_receiver(
        &mut self,
    ) -> Option<mpsc::UnboundedReceiver<ProcessDataReceivedEventArgs>> {
        self.stderr_rx.take()
    }

    /// Execute a process with the given parameters.
    ///
    /// # Arguments
    /// * `working_directory` - The working directory for the process.
    /// * `file_name` - The executable to run.
    /// * `arguments` - Command-line arguments as a single string.
    /// * `environment` - Optional environment variable overrides.
    /// * `require_exit_code_zero` - If true, returns an error on non-zero exit.
    /// * `kill_process_on_cancel` - If true, skip graceful shutdown and SIGKILL immediately.
    /// * `cancellation_token` - Token to cancel/kill the process.
    ///
    /// Returns the process exit code.
    pub async fn execute(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: Option<&HashMap<String, String>>,
        require_exit_code_zero: bool,
        kill_process_on_cancel: bool,
        cancellation_token: CancellationToken,
    ) -> Result<i32> {
        assert!(!file_name.is_empty(), "file_name must not be empty");

        self.trace.info("Starting process:");
        self.trace
            .info(&format!("  File name: '{file_name}'"));
        self.trace
            .info(&format!("  Arguments: '{arguments}'"));
        self.trace
            .info(&format!("  Working directory: '{working_directory}'"));
        self.trace.info(&format!(
            "  Require exit code zero: '{require_exit_code_zero}'"
        ));
        self.trace.info(&format!(
            "  Force kill process on cancellation: '{kill_process_on_cancel}'"
        ));

        let mut cmd = Command::new(file_name);

        // Split arguments by whitespace for argument passing.
        // The C# version passes a single arguments string to ProcessStartInfo.Arguments.
        // We do a simple shell-like split for cross-platform compatibility.
        if !arguments.is_empty() {
            for arg in shell_split(arguments) {
                cmd.arg(arg);
            }
        }

        if !working_directory.is_empty() && Path::new(working_directory).is_dir() {
            cmd.current_dir(working_directory);
        }

        // Set environment variables
        if let Some(env) = environment {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }

        // Always set GITHUB_ACTIONS=true
        cmd.env("GITHUB_ACTIONS", "true");

        // Set CI=true if not already set
        if std::env::var("CI").is_err() {
            if let Some(env) = environment {
                if !env.contains_key("CI") {
                    cmd.env("CI", "true");
                }
            } else {
                cmd.env("CI", "true");
            }
        }

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.stdin(std::process::Stdio::null());

        let start = std::time::Instant::now();
        let mut child = cmd.spawn().with_context(|| {
            format!("Failed to start process '{file_name}' with arguments '{arguments}'")
        })?;

        let pid = child.id().unwrap_or(0);
        self.trace
            .info(&format!("Process started with process id {pid}, waiting for process exit."));

        // Spawn stdout reader
        let stdout = child.stdout.take();
        let stdout_tx = self.stdout_tx.clone();
        let trace_clone = self.trace.clone();
        let stdout_task = tokio::spawn(async move {
            if let Some(stdout) = stdout {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = stdout_tx.send(ProcessDataReceivedEventArgs { data: line });
                }
            }
            trace_clone.info("STDOUT stream read finished.");
        });

        // Spawn stderr reader
        let stderr = child.stderr.take();
        let stderr_tx = self.stderr_tx.clone();
        let trace_clone2 = self.trace.clone();
        let stderr_task = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = stderr_tx.send(ProcessDataReceivedEventArgs { data: line });
                }
            }
            trace_clone2.info("STDERR stream read finished.");
        });

        // Wait for process exit or cancellation
        let exit_code: i32;
        let was_cancelled;

        tokio::select! {
            status = child.wait() => {
                was_cancelled = false;
                match status {
                    Ok(s) => {
                        exit_code = s.code().unwrap_or(-1);
                    }
                    Err(e) => {
                        return Err(e).context("Failed to wait for process");
                    }
                }
            }
            _ = cancellation_token.cancelled() => {
                was_cancelled = true;
                self.trace.info("Cancellation requested.");
                exit_code = self.cancel_and_kill_process(&mut child, kill_process_on_cancel).await;
            }
        }

        // Wait for stream readers to finish
        let _ = stdout_task.await;
        let _ = stderr_task.await;

        let elapsed = start.elapsed();
        self.trace.info(&format!(
            "Finished process {pid} with exit code {exit_code}, and elapsed time {elapsed:.2?}."
        ));

        if was_cancelled {
            anyhow::bail!("Process was cancelled");
        }

        if exit_code != 0 && require_exit_code_zero {
            return Err(ProcessExitCodeError {
                exit_code,
                file_name: file_name.to_string(),
                arguments: arguments.to_string(),
            }
            .into());
        }

        Ok(exit_code)
    }

    /// Attempt graceful cancellation: SIGINT → SIGTERM → SIGKILL.
    /// If `kill_immediately` is true, skip signals and go straight to kill.
    async fn cancel_and_kill_process(
        &self,
        child: &mut tokio::process::Child,
        kill_immediately: bool,
    ) -> i32 {
        if !kill_immediately {
            // Try SIGINT first
            if self.send_signal_and_wait(child, Signal::Int, SIGINT_TIMEOUT).await {
                self.trace
                    .info("Process cancelled successfully through SIGINT.");
                return child
                    .wait()
                    .await
                    .map(|s| s.code().unwrap_or(-1))
                    .unwrap_or(-1);
            }

            // Try SIGTERM
            if self.send_signal_and_wait(child, Signal::Term, SIGTERM_TIMEOUT).await {
                self.trace
                    .info("Process terminated successfully through SIGTERM.");
                return child
                    .wait()
                    .await
                    .map(|s| s.code().unwrap_or(-1))
                    .unwrap_or(-1);
            }
        }

        // Force kill
        self.trace.info(
            "Kill entire process tree since both cancel and terminate signals have been ignored.",
        );
        let _ = child.kill().await;
        child
            .wait()
            .await
            .map(|s| s.code().unwrap_or(-1))
            .unwrap_or(-1)
    }

    /// Send a signal to the child process and wait up to `timeout` for it to exit.
    /// Returns `true` if the process exited within the timeout.
    #[cfg(unix)]
    async fn send_signal_and_wait(
        &self,
        child: &mut tokio::process::Child,
        signal: Signal,
        timeout: Duration,
    ) -> bool {
        let pid = match child.id() {
            Some(id) => id,
            None => {
                // Process already exited
                return true;
            }
        };

        let sig = match signal {
            Signal::Int => nix::sys::signal::Signal::SIGINT,
            Signal::Term => nix::sys::signal::Signal::SIGTERM,
        };

        self.trace.info(&format!(
            "Sending {sig:?} to process {pid}."
        ));

        let send_result =
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), sig);
        if send_result.is_err() {
            self.trace.info(&format!(
                "{sig:?} signal failed to send to process {pid}."
            ));
            return false;
        }

        self.trace.info(&format!(
            "Waiting for process exit or {:.1}s after {sig:?} signal.",
            timeout.as_secs_f64()
        ));

        tokio::select! {
            result = child.wait() => {
                result.is_ok()
            }
            _ = tokio::time::sleep(timeout) => {
                self.trace.info(&format!(
                    "Process did not honor {sig:?} within {:.1}s.",
                    timeout.as_secs_f64()
                ));
                false
            }
        }
    }

    #[cfg(not(unix))]
    async fn send_signal_and_wait(
        &self,
        child: &mut tokio::process::Child,
        _signal: Signal,
        timeout: Duration,
    ) -> bool {
        // On Windows, there's no direct POSIX signal. We attempt to wait
        // with a timeout and then force kill if needed.
        tokio::select! {
            result = child.wait() => {
                result.is_ok()
            }
            _ = tokio::time::sleep(timeout) => {
                false
            }
        }
    }
}

/// Internal signal type for cross-platform abstraction.
#[derive(Debug, Clone, Copy)]
enum Signal {
    Int,
    Term,
}

/// Simple argument splitting. Splits on whitespace but respects double-quoted
/// and single-quoted strings. This is a minimal implementation; for production
/// use, consider the `shell-words` crate.
fn shell_split(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;

    for ch in input.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if !in_single_quote => {
                escape_next = true;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::NullTraceWriter;

    fn make_invoker() -> ProcessInvoker {
        ProcessInvoker::new(Arc::new(NullTraceWriter))
    }

    #[test]
    fn shell_split_simple() {
        let args = shell_split("hello world");
        assert_eq!(args, vec!["hello", "world"]);
    }

    #[test]
    fn shell_split_quoted() {
        let args = shell_split(r#"hello "world foo" bar"#);
        assert_eq!(args, vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn shell_split_single_quoted() {
        let args = shell_split("hello 'world foo' bar");
        assert_eq!(args, vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn shell_split_empty() {
        let args = shell_split("");
        assert!(args.is_empty());
    }

    #[tokio::test]
    async fn execute_echo() {
        let mut invoker = make_invoker();
        let mut rx = invoker.take_stdout_receiver().unwrap();
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(async move {
            invoker
                .execute(
                    "",
                    "echo",
                    "hello",
                    None,
                    false,
                    false,
                    cancel,
                )
                .await
        });

        // Collect stdout
        let mut lines = Vec::new();
        while let Some(evt) = rx.recv().await {
            lines.push(evt.data);
        }

        let exit_code = handle.await.unwrap().unwrap();
        assert_eq!(exit_code, 0);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("hello"));
    }

    #[tokio::test]
    async fn execute_nonexistent() {
        let invoker = make_invoker();
        let cancel = CancellationToken::new();
        let result = invoker
            .execute(
                "",
                "nonexistent_command_xyz_123",
                "",
                None,
                false,
                false,
                cancel,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_require_exit_code_zero() {
        let invoker = make_invoker();
        let cancel = CancellationToken::new();
        let result = invoker
            .execute(
                "",
                "false",
                "",
                None,
                true, // require exit code zero
                false,
                cancel,
            )
            .await;
        assert!(result.is_err());
        let err_str = format!("{}", result.unwrap_err());
        assert!(err_str.contains("Exit code"));
    }

    #[tokio::test]
    async fn execute_with_env() {
        let mut env = HashMap::new();
        env.insert("MY_TEST_VAR".to_string(), "test_value_123".to_string());

        let mut invoker = make_invoker();
        let mut rx = invoker.take_stdout_receiver().unwrap();
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(async move {
            invoker
                .execute(
                    "",
                    "sh",
                    "-c echo $MY_TEST_VAR",
                    Some(&env),
                    false,
                    false,
                    cancel,
                )
                .await
        });

        let mut lines = Vec::new();
        while let Some(evt) = rx.recv().await {
            lines.push(evt.data);
        }

        let exit_code = handle.await.unwrap().unwrap();
        assert_eq!(exit_code, 0);
    }
}
