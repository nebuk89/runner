// ContainerOperationProvider mapping `ContainerOperationProvider.cs`.
// High-level operations for starting/stopping job and service containers,
// creating Docker networks, and managing container hooks.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use runner_common::constants;
use runner_common::host_context::HostContext;

use crate::container::container_info::ContainerInfo;
use crate::container::docker_command_manager::DockerCommandManager;
use crate::execution_context::ExecutionContext;
use crate::worker::{AgentJobRequestMessage, JobContainerInfo};

/// Provides high-level container lifecycle operations for the job.
pub struct ContainerOperationProvider {
    docker: DockerCommandManager,
}

impl ContainerOperationProvider {
    /// Create a new provider.
    pub fn new() -> Self {
        Self {
            docker: DockerCommandManager::new(),
        }
    }

    /// Create a new provider with a custom Docker command manager.
    pub fn with_docker(docker: DockerCommandManager) -> Self {
        Self { docker }
    }

    /// Set up the job container and service containers.
    ///
    /// 1. Creates a Docker network for the job
    /// 2. Starts service containers
    /// 3. Starts the job container
    /// 4. Updates the execution context with container info
    pub async fn start_containers_async(
        &self,
        context: &mut ExecutionContext,
        message: &AgentJobRequestMessage,
    ) -> Result<()> {
        let job_id = &message.job_id;

        // Create a network for the job
        let network_name = format!("github_network_{}", job_id);
        context.info(&format!("Creating Docker network: {}", network_name));

        let network_id = self
            .docker
            .create_network(&network_name, context.cancel_token())
            .await
            .context("Failed to create Docker network")?;

        context.debug(&format!("Network created: {}", network_id));

        // Start service containers
        // TODO: Parse service containers from TemplateToken format.
        // For now, TemplateToken-based containers are not supported.
        if message.has_service_containers() {
            context.warning(
                "Service containers in TemplateToken format are not yet supported \
                 in the Rust runner. Skipping service container startup.",
            );
        }

        // Start the job container if defined
        // TODO: Parse job container from TemplateToken format.
        if message.has_job_container() {
            context.warning(
                "Job container in TemplateToken format is not yet supported \
                 in the Rust runner. Skipping job container startup.",
            );
        }

        Ok(())
    }

    /// Stop and remove all containers and the job network.
    pub async fn stop_containers_async(
        &self,
        context: &mut ExecutionContext,
    ) -> Result<()> {
        let cancel = context.cancel_token();

        // Stop job container
        let container_info = context.global().container_info.clone();
        if let Some(ref container) = container_info {
            if let Some(ref id) = container.container_id {
                context.info(&format!("Stopping job container: {}", id));
                let _ = self.docker.stop_container(id, cancel.clone()).await;
                let _ = self.docker.remove_container(id, cancel.clone()).await;
            }
        }

        // Stop service containers
        let service_containers: Vec<_> = context.global().service_containers.clone();
        for container in &service_containers {
            if let Some(ref id) = container.container_id {
                context.info(&format!("Stopping service container: {}", id));
                let _ = self.docker.stop_container(id, cancel.clone()).await;
                let _ = self.docker.remove_container(id, cancel.clone()).await;
            }
        }

        // Remove network
        let container_info2 = context.global().container_info.clone();
        if let Some(ref container) = container_info2 {
            if let Some(ref network) = container.network {
                context.info(&format!("Removing Docker network: {}", network));
                let _ = self.docker.remove_network(network, cancel.clone()).await;
            }
        }

        // Clear container info
        context.global_mut().container_info = None;
        context.global_mut().service_containers.clear();

        Ok(())
    }

    /// Start a single service container.
    async fn start_service_container(
        &self,
        context: &mut ExecutionContext,
        definition: &JobContainerInfo,
        name: &str,
        network: &str,
    ) -> Result<ContainerInfo> {
        // Pull the image
        self.docker
            .pull_image(&definition.image, context.cancel_token())
            .await?;

        // Build container info
        let mut container = ContainerInfo::new(&definition.image);
        container.container_name = name.to_string();
        container.network = Some(network.to_string());
        container.container_network_alias = Some(name.to_string());
        container.environment = definition.environment.clone();
        container.volumes = definition.volumes.clone();
        container.ports = definition.ports.clone();
        container.options = definition.options.clone();

        // Login if credentials provided
        if let Some(ref creds) = definition.credentials {
            if !creds.username.is_empty() {
                self.docker
                    .docker_login("", &creds.username, &creds.password, context.cancel_token())
                    .await?;
            }
        }

        // Create and start
        let container_id = self
            .docker
            .create_container(&container, context.cancel_token())
            .await?;

        container.container_id = Some(container_id.clone());

        self.docker
            .start_container(&container_id, context.cancel_token())
            .await?;

        context.info(&format!(
            "Service container {} ({}) started: {}",
            name,
            definition.image,
            &container_id[..12.min(container_id.len())]
        ));

        Ok(container)
    }

    /// Start the job container.
    async fn start_job_container(
        &self,
        context: &mut ExecutionContext,
        definition: &JobContainerInfo,
        job_id: &str,
        network: &str,
    ) -> Result<ContainerInfo> {
        // Pull the image
        self.docker
            .pull_image(&definition.image, context.cancel_token())
            .await?;

        let container_name = format!("runner_job_{}", job_id);

        let mut container = ContainerInfo::new(&definition.image);
        container.container_name = container_name;
        container.network = Some(network.to_string());
        container.is_job_container = true;
        container.environment = definition.environment.clone();
        container.volumes = definition.volumes.clone();
        container.ports = definition.ports.clone();
        container.options = definition.options.clone();

        // Add workspace volume mount
        let workspace = context.global().workspace_directory.clone();
        container
            .volumes
            .push(format!("{}:/github/workspace", workspace));

        // Set up path mappings
        container.path_mappings.insert(
            workspace.clone(),
            "/github/workspace".to_string(),
        );

        // Set entrypoint to keep container running
        container.entrypoint = Some("tail".to_string());

        // Login if credentials provided
        if let Some(ref creds) = definition.credentials {
            if !creds.username.is_empty() {
                self.docker
                    .docker_login("", &creds.username, &creds.password, context.cancel_token())
                    .await?;
            }
        }

        // Create and start
        let container_id = self
            .docker
            .create_container(&container, context.cancel_token())
            .await?;

        container.container_id = Some(container_id.clone());

        self.docker
            .start_container(&container_id, context.cancel_token())
            .await?;

        context.info(&format!(
            "Job container started: {}",
            &container_id[..12.min(container_id.len())]
        ));

        Ok(container)
    }

    /// Check if container hooks are configured.
    pub fn is_container_hooks_enabled() -> bool {
        std::env::var(constants::hooks::CONTAINER_HOOKS_PATH)
            .ok()
            .filter(|p| !p.is_empty())
            .is_some()
    }
}

impl Default for ContainerOperationProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = ContainerOperationProvider::new();
        let _ = provider;
    }

    #[test]
    fn test_container_hooks_check() {
        // In test env, hooks should not be enabled
        let _ = ContainerOperationProvider::is_container_hooks_enabled();
    }
}
