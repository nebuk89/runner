// StepHost mapping `StepHost.cs`.
// Defines the interface for executing processes on the host or in a container.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use runner_sdk::ProcessInvoker;
use runner_sdk::TraceWriter;

/// Result of a step execution including the exit code and captured output lines.
pub struct StepHostOutput {
    /// The process exit code.
    pub exit_code: i32,
    /// All stdout and stderr lines captured during execution, in order.
    pub output_lines: Vec<String>,
}

/// Trait for step execution hosts.
///
/// `DefaultStepHost` runs processes directly on the host.
/// `ContainerStepHost` runs processes inside a Docker container via `docker exec`.
#[async_trait]
pub trait StepHost: Send + Sync {
    /// Execute a process.
    ///
    /// Returns the exit code and all captured output lines.
    async fn execute_async(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: &HashMap<String, String>,
        cancel_token: CancellationToken,
    ) -> Result<StepHostOutput>;
}

/// Default step host - runs processes directly on the host OS.
pub struct DefaultStepHost;

impl DefaultStepHost {
    pub fn new() -> Self {
        Self
    }
}

/// A simple trace writer that logs via tracing.
struct StepHostTraceWriter;

impl TraceWriter for StepHostTraceWriter {
    fn info(&self, message: &str) {
        tracing::info!(target: "step_host", "{}", message);
    }

    fn verbose(&self, message: &str) {
        tracing::debug!(target: "step_host", "{}", message);
    }

    fn error(&self, message: &str) {
        tracing::error!(target: "step_host", "{}", message);
    }
}

#[async_trait]
impl StepHost for DefaultStepHost {
    async fn execute_async(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: &HashMap<String, String>,
        cancel_token: CancellationToken,
    ) -> Result<StepHostOutput> {
        let trace = std::sync::Arc::new(StepHostTraceWriter);
        let mut invoker = ProcessInvoker::new(trace);

        // Take the output receivers so we can capture lines
        let mut stdout_rx = invoker.take_stdout_receiver();
        let mut stderr_rx = invoker.take_stderr_receiver();

        // Collect output lines in a shared vec
        let output_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        // Spawn tasks to read stdout and stderr into our collection
        let out_lines = output_lines.clone();
        let stdout_task = tokio::spawn(async move {
            if let Some(ref mut rx) = stdout_rx {
                while let Some(event) = rx.recv().await {
                    tracing::info!(target: "step_host", "{}", event.data);
                    out_lines.lock().unwrap().push(event.data);
                }
            }
        });

        let err_lines = output_lines.clone();
        let stderr_task = tokio::spawn(async move {
            if let Some(ref mut rx) = stderr_rx {
                while let Some(event) = rx.recv().await {
                    tracing::info!(target: "step_host", "{}", event.data);
                    err_lines.lock().unwrap().push(event.data);
                }
            }
        });

        let exit_code = invoker
            .execute(
                working_directory,
                file_name,
                arguments,
                Some(environment),
                false, // don't require exit code zero - we handle it ourselves
                false, // don't kill on cancel immediately
                cancel_token,
            )
            .await
            .context("Process execution failed")?;

        // Drop the invoker to close the channel senders, so the receiver tasks can finish
        drop(invoker);

        // Wait for output readers to finish
        let _ = stdout_task.await;
        let _ = stderr_task.await;

        let lines = match std::sync::Arc::try_unwrap(output_lines) {
            Ok(mutex) => mutex.into_inner().unwrap(),
            Err(arc) => arc.lock().unwrap().clone(),
        };

        Ok(StepHostOutput {
            exit_code,
            output_lines: lines,
        })
    }
}

/// Container step host - runs processes inside a Docker container via `docker exec`.
pub struct ContainerStepHost {
    container_id: String,
}

impl ContainerStepHost {
    pub fn new(container_id: String) -> Self {
        Self { container_id }
    }
}

#[async_trait]
impl StepHost for ContainerStepHost {
    async fn execute_async(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: &HashMap<String, String>,
        cancel_token: CancellationToken,
    ) -> Result<StepHostOutput> {
        let trace = std::sync::Arc::new(StepHostTraceWriter);

        // Build docker exec command
        let mut docker_args = vec![
            "exec".to_string(),
        ];

        // Add environment variables
        for (key, value) in environment {
            docker_args.push("-e".to_string());
            docker_args.push(format!("{}={}", key, value));
        }

        // Set working directory
        if !working_directory.is_empty() {
            docker_args.push("-w".to_string());
            docker_args.push(working_directory.to_string());
        }

        // Container ID
        docker_args.push(self.container_id.clone());

        // Command to execute
        docker_args.push(file_name.to_string());
        if !arguments.is_empty() {
            for arg in arguments.split_whitespace() {
                docker_args.push(arg.to_string());
            }
        }

        let docker_arguments = docker_args.join(" ");

        let mut invoker = ProcessInvoker::new(trace);

        // Capture output
        let mut stdout_rx = invoker.take_stdout_receiver();
        let mut stderr_rx = invoker.take_stderr_receiver();
        let output_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        let out_lines = output_lines.clone();
        let stdout_task = tokio::spawn(async move {
            if let Some(ref mut rx) = stdout_rx {
                while let Some(event) = rx.recv().await {
                    tracing::info!(target: "step_host", "{}", event.data);
                    out_lines.lock().unwrap().push(event.data);
                }
            }
        });

        let err_lines = output_lines.clone();
        let stderr_task = tokio::spawn(async move {
            if let Some(ref mut rx) = stderr_rx {
                while let Some(event) = rx.recv().await {
                    tracing::info!(target: "step_host", "{}", event.data);
                    err_lines.lock().unwrap().push(event.data);
                }
            }
        });

        let exit_code = invoker
            .execute(
                "",
                "docker",
                &docker_arguments,
                None,
                false,
                false,
                cancel_token,
            )
            .await
            .context("Docker exec failed")?;

        // Drop the invoker to close the channel senders
        drop(invoker);

        let _ = stdout_task.await;
        let _ = stderr_task.await;

        let lines = match std::sync::Arc::try_unwrap(output_lines) {
            Ok(mutex) => mutex.into_inner().unwrap(),
            Err(arc) => arc.lock().unwrap().clone(),
        };

        Ok(StepHostOutput {
            exit_code,
            output_lines: lines,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_step_host_creation() {
        let host = DefaultStepHost::new();
        let _ = host;
    }

    #[test]
    fn test_container_step_host_creation() {
        let host = ContainerStepHost::new("abc123".to_string());
        assert_eq!(host.container_id, "abc123");
    }
}
