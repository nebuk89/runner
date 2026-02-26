// ContainerActionHandler mapping `ContainerActionHandler.cs`.
// Executes Docker container-based actions.

use async_trait::async_trait;
use anyhow::{Context, Result};
use std::collections::HashMap;

use runner_common::constants::{CURRENT_PLATFORM, OsPlatform};

use crate::container::container_info::ContainerInfo;
use crate::execution_context::ExecutionContext;
use crate::handlers::handler::{Handler, HandlerData};

/// Handler for Docker container-based actions.
pub struct ContainerActionHandler;

impl ContainerActionHandler {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Handler for ContainerActionHandler {
    async fn run_async(
        &self,
        context: &mut ExecutionContext,
        data: &HandlerData,
    ) -> Result<()> {
        self.prepare_execution(context, data);

        // Container actions only work on Linux
        if CURRENT_PLATFORM != OsPlatform::Linux {
            context.error(
                "Container actions are only supported on Linux runners. \
                 Use a different action type or switch to a Linux runner.",
            );
            context.complete(
                runner_common::util::task_result_util::TaskResult::Failed,
                Some("Container actions require Linux"),
            );
            return Ok(());
        }

        // Determine if we need to build or pull
        let image = if let Some(ref dockerfile) = data.action_context.dockerfile {
            // Build the image from a Dockerfile
            let action_dir = &data.action_context.action_directory;
            let image_tag = format!(
                "act-{}-{}",
                context.global().job_id,
                context
                    .current_step_id()
                    .unwrap_or("unknown")
                    .to_lowercase()
            );

            context.info(&format!("Building Docker image from {}", dockerfile));

            let docker = crate::container::docker_command_manager::DockerCommandManager::new();
            docker
                .build_image(action_dir, dockerfile, &image_tag, context.cancel_token())
                .await
                .context("Failed to build Docker image")?;

            image_tag
        } else if let Some(ref image) = data.action_context.image {
            // Pull the image
            context.info(&format!("Pulling Docker image: {}", image));

            let docker = crate::container::docker_command_manager::DockerCommandManager::new();
            docker
                .pull_image(image, context.cancel_token())
                .await
                .context("Failed to pull Docker image")?;

            image.clone()
        } else {
            context.error("Container action has no image or Dockerfile configured.");
            context.complete(
                runner_common::util::task_result_util::TaskResult::Failed,
                Some("No container image specified"),
            );
            return Ok(());
        };

        // Build environment for the container
        let mut env = context.global().environment_variables.clone();
        for (k, v) in &context.step_environment {
            env.insert(k.clone(), v.clone());
        }
        for (key, value) in &data.inputs {
            let env_name = format!("INPUT_{}", key.to_uppercase().replace(' ', "_"));
            env.insert(env_name, value.clone());
        }

        // Build the entrypoint and args
        let entrypoint = data
            .action_context
            .entry_point
            .clone();

        let args: Vec<String> = data
            .inputs
            .get("args")
            .map(|a| {
                a.split_whitespace()
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        // Build volume mounts
        let workspace = context.global().workspace_directory.clone();
        let mut volumes = vec![
            format!("{}:/github/workspace", workspace),
        ];

        // Create ContainerInfo
        let container = ContainerInfo {
            image: image.clone(),
            container_id: None,
            container_name: format!(
                "runner-{}-{}",
                context.global().job_id,
                context.current_step_id().unwrap_or("step")
            ),
            network: None,
            entrypoint: if entrypoint.is_empty() {
                None
            } else {
                Some(entrypoint)
            },
            environment: env.clone(),
            volumes,
            ports: Vec::new(),
            options: None,
            path_mappings: HashMap::new(),
            is_job_container: false,
            container_network_alias: None,
            user_mountvolumes: Vec::new(),
        };

        // Create and start the container
        let docker = crate::container::docker_command_manager::DockerCommandManager::new();

        context.info("Creating container...");
        let container_id = docker
            .create_container(&container, context.cancel_token())
            .await
            .context("Failed to create container")?;

        context.info(&format!("Starting container {}...", &container_id[..12]));
        docker
            .start_container(&container_id, context.cancel_token())
            .await
            .context("Failed to start container")?;

        // Wait for the container to finish
        let exit_code = docker
            .wait_container(&container_id, context.cancel_token())
            .await
            .context("Failed to wait for container")?;

        // Collect logs
        let logs = docker
            .get_container_logs(&container_id, context.cancel_token())
            .await
            .unwrap_or_default();

        for line in logs.lines() {
            context.write(line);
        }

        // Clean up container
        let _ = docker
            .remove_container(&container_id, context.cancel_token())
            .await;

        // Handle exit code
        if exit_code != 0 {
            context.error(&format!(
                "Container action completed with exit code {}.",
                exit_code
            ));
            context.complete(
                runner_common::util::task_result_util::TaskResult::Failed,
                Some(&format!("Container exit code {}", exit_code)),
            );
        }

        context.end_section();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_handler_creation() {
        let handler = ContainerActionHandler::new();
        let _ = handler;
    }
}
