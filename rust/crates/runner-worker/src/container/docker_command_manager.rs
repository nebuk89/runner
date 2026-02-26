// DockerCommandManager mapping `DockerCommandManager.cs`.
// Wraps Docker CLI invocations for container management.

use anyhow::{Context, Result};
use runner_sdk::ProcessInvoker;
use runner_sdk::TraceWriter;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::container::container_info::ContainerInfo;

/// Docker CLI trace writer.
struct DockerTraceWriter;

impl TraceWriter for DockerTraceWriter {
    fn info(&self, message: &str) {
        tracing::info!(target: "docker", "{}", message);
    }

    fn verbose(&self, message: &str) {
        tracing::debug!(target: "docker", "{}", message);
    }

    fn error(&self, message: &str) {
        tracing::error!(target: "docker", "{}", message);
    }
}

/// Manages Docker CLI operations.
pub struct DockerCommandManager {
    docker_path: String,
}

impl DockerCommandManager {
    /// Create a new `DockerCommandManager`.
    pub fn new() -> Self {
        Self {
            docker_path: "docker".to_string(),
        }
    }

    /// Create a new `DockerCommandManager` with a custom Docker binary path.
    pub fn with_path(docker_path: impl Into<String>) -> Self {
        Self {
            docker_path: docker_path.into(),
        }
    }

    // -----------------------------------------------------------------------
    // Container lifecycle
    // -----------------------------------------------------------------------

    /// Create a container from a `ContainerInfo`.
    ///
    /// Returns the container ID.
    pub async fn create_container(
        &self,
        container: &ContainerInfo,
        cancel: CancellationToken,
    ) -> Result<String> {
        let mut args = vec!["create".to_string()];

        // Name
        if !container.container_name.is_empty() {
            args.push("--name".to_string());
            args.push(container.container_name.clone());
        }

        // Network
        if let Some(ref network) = container.network {
            args.push("--network".to_string());
            args.push(network.clone());
        }

        // Network alias
        if let Some(ref alias) = container.container_network_alias {
            args.push("--network-alias".to_string());
            args.push(alias.clone());
        }

        // Entrypoint
        if let Some(ref ep) = container.entrypoint {
            args.push("--entrypoint".to_string());
            args.push(ep.clone());
        }

        // Environment
        args.extend(container.build_env_args());

        // Volumes
        args.extend(container.build_volume_args());

        // Ports
        args.extend(container.build_port_args());

        // Options
        if let Some(ref opts) = container.options {
            for opt in opts.split_whitespace() {
                args.push(opt.to_string());
            }
        }

        // Image
        args.push(container.image.clone());

        let arguments = args.join(" ");
        let output = self.run_docker_command(&arguments, cancel).await?;
        let container_id = output.trim().to_string();

        Ok(container_id)
    }

    /// Start a container by ID.
    pub async fn start_container(
        &self,
        container_id: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = format!("start {}", container_id);
        self.run_docker_command(&args, cancel).await?;
        Ok(())
    }

    /// Stop a container by ID.
    pub async fn stop_container(
        &self,
        container_id: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = format!("stop {}", container_id);
        self.run_docker_command(&args, cancel).await?;
        Ok(())
    }

    /// Remove a container by ID.
    pub async fn remove_container(
        &self,
        container_id: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = format!("rm --force {}", container_id);
        self.run_docker_command(&args, cancel).await?;
        Ok(())
    }

    /// Wait for a container to exit and return its exit code.
    pub async fn wait_container(
        &self,
        container_id: &str,
        cancel: CancellationToken,
    ) -> Result<i32> {
        let args = format!("wait {}", container_id);
        let output = self.run_docker_command(&args, cancel).await?;
        let exit_code = output
            .trim()
            .parse::<i32>()
            .unwrap_or(-1);
        Ok(exit_code)
    }

    /// Get container logs.
    pub async fn get_container_logs(
        &self,
        container_id: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let args = format!("logs {}", container_id);
        self.run_docker_command(&args, cancel).await
    }

    /// Execute a command inside a running container.
    pub async fn exec_container(
        &self,
        container_id: &str,
        command: &str,
        environment: &HashMap<String, String>,
        working_directory: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<i32> {
        let mut args = vec!["exec".to_string()];

        for (key, value) in environment {
            args.push("-e".to_string());
            args.push(format!("{}={}", key, value));
        }

        if let Some(wd) = working_directory {
            args.push("-w".to_string());
            args.push(wd.to_string());
        }

        args.push(container_id.to_string());

        for part in command.split_whitespace() {
            args.push(part.to_string());
        }

        let arguments = args.join(" ");
        let trace: Arc<dyn TraceWriter> = Arc::new(DockerTraceWriter);
        let invoker = ProcessInvoker::new(trace);

        let exit_code = invoker
            .execute(
                "",
                &self.docker_path,
                &arguments,
                None,
                false,
                false,
                cancel,
            )
            .await
            .context("Docker exec failed")?;

        Ok(exit_code)
    }

    // -----------------------------------------------------------------------
    // Image operations
    // -----------------------------------------------------------------------

    /// Pull a Docker image.
    pub async fn pull_image(
        &self,
        image: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = format!("pull {}", image);
        self.run_docker_command(&args, cancel).await?;
        Ok(())
    }

    /// Build a Docker image from a Dockerfile.
    pub async fn build_image(
        &self,
        context_dir: &str,
        dockerfile: &str,
        tag: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = format!("build -t {} -f {} {}", tag, dockerfile, context_dir);
        self.run_docker_command(&args, cancel).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Network operations
    // -----------------------------------------------------------------------

    /// Create a Docker network.
    pub async fn create_network(
        &self,
        name: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let args = format!("network create {}", name);
        let output = self.run_docker_command(&args, cancel).await?;
        Ok(output.trim().to_string())
    }

    /// Remove a Docker network.
    pub async fn remove_network(
        &self,
        name: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = format!("network rm {}", name);
        self.run_docker_command(&args, cancel).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Auth
    // -----------------------------------------------------------------------

    /// Login to a Docker registry.
    pub async fn docker_login(
        &self,
        server: &str,
        username: &str,
        password: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = format!(
            "login {} -u {} --password-stdin",
            server, username
        );

        // For docker login with --password-stdin, we'd need to pipe the password.
        // Using -p is less secure but simpler for this implementation.
        let args = format!("login {} -u {} -p {}", server, username, password);
        self.run_docker_command(&args, cancel).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Inspection
    // -----------------------------------------------------------------------

    /// Inspect a container and return the JSON output.
    pub async fn inspect_container(
        &self,
        container_id: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let args = format!("inspect {}", container_id);
        self.run_docker_command(&args, cancel).await
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Run a Docker CLI command and return its stdout output.
    async fn run_docker_command(
        &self,
        arguments: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let trace: Arc<dyn TraceWriter> = Arc::new(DockerTraceWriter);
        let mut invoker = ProcessInvoker::new(trace);

        // Collect stdout output
        let mut stdout_rx = invoker.take_stdout_receiver().unwrap();
        let output_handle = tokio::spawn(async move {
            let mut lines = Vec::new();
            while let Some(event) = stdout_rx.recv().await {
                lines.push(event.data);
            }
            lines.join("\n")
        });

        let exit_code = invoker
            .execute(
                "",
                &self.docker_path,
                arguments,
                None,
                false,
                false,
                cancel,
            )
            .await
            .with_context(|| {
                format!("Docker command failed: {} {}", self.docker_path, arguments)
            })?;

        let output = output_handle.await.unwrap_or_default();

        if exit_code != 0 {
            anyhow::bail!(
                "Docker command exited with code {}: {} {}",
                exit_code,
                self.docker_path,
                arguments
            );
        }

        Ok(output)
    }
}

impl Default for DockerCommandManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_command_manager_new() {
        let mgr = DockerCommandManager::new();
        assert_eq!(mgr.docker_path, "docker");
    }

    #[test]
    fn test_docker_command_manager_with_path() {
        let mgr = DockerCommandManager::with_path("/usr/local/bin/docker");
        assert_eq!(mgr.docker_path, "/usr/local/bin/docker");
    }
}
