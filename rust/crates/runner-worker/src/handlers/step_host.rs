// StepHost mapping `StepHost.cs`.
// Defines the interface for executing processes on the host or in a container.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use runner_sdk::ProcessInvoker;
use runner_sdk::TraceWriter;

/// Trait for step execution hosts.
///
/// `DefaultStepHost` runs processes directly on the host.
/// `ContainerStepHost` runs processes inside a Docker container via `docker exec`.
#[async_trait]
pub trait StepHost: Send + Sync {
    /// Execute a process.
    ///
    /// Returns the process exit code.
    async fn execute_async(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: &HashMap<String, String>,
        cancel_token: CancellationToken,
    ) -> Result<i32>;
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
    ) -> Result<i32> {
        let trace = std::sync::Arc::new(StepHostTraceWriter);
        let invoker = ProcessInvoker::new(trace);

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

        Ok(exit_code)
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
    ) -> Result<i32> {
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

        let invoker = ProcessInvoker::new(trace);
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

        Ok(exit_code)
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
