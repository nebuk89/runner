// JobRunner mapping `JobRunner.cs`.
// Orchestrates a single job execution: sets up the execution context,
// delegates to the job extension for step building, invokes the steps runner,
// and reports the final result back to the server.

use anyhow::{Context, Result};
use runner_common::host_context::HostContext;
use runner_common::util::task_result_util::TaskResult;
use runner_common::util::var_util::VarUtil;
use runner_sdk::TraceWriter;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::execution_context::{ExecutionContext, Global};
use crate::feature_manager::FeatureManager;
use crate::job_extension::JobExtension;
use crate::steps_runner::StepsRunner;
use crate::tracking_manager::TrackingManager;
use crate::variables::Variables;
use crate::worker::AgentJobRequestMessage;

/// Orchestrates a single job execution.
pub struct JobRunner {
    host_context: Arc<HostContext>,
}

impl JobRunner {
    /// Create a new `JobRunner`.
    pub fn new(host_context: Arc<HostContext>) -> Self {
        Self { host_context }
    }

    /// Run a job to completion.
    ///
    /// 1. Creates the root `ExecutionContext`
    /// 2. Sets runner context (os, arch, name, tool_cache)
    /// 3. Establishes server connection info
    /// 4. Delegates to `JobExtension::initialize_job` for step building
    /// 5. Invokes `StepsRunner::run_async` to execute steps
    /// 6. Calls `JobExtension::finalize_job` for cleanup
    /// 7. Returns the final `TaskResult`
    pub async fn run_async(
        &self,
        message: AgentJobRequestMessage,
        cancel_token: CancellationToken,
    ) -> Result<TaskResult> {
        let trace = self.host_context.get_trace("JobRunner");
        trace.info(&format!(
            "Starting job: {} ({})",
            message.job_display_name, message.job_id
        ));

        // Build Variables from the job message
        let variables = Variables::from_message(&message, &self.host_context.secret_masker);

        // Determine the pipeline directory using TrackingManager
        let tracking_manager = TrackingManager::new(&self.host_context);
        let (pipeline_directory, workspace_directory, _temp_directory) = tracking_manager
            .prepare_pipeline_directory(&message)
            .unwrap_or_else(|e| {
                trace.info(&format!("Failed to prepare pipeline directory: {}", e));
                let fallback = self.host_context
                    .get_directory(runner_common::constants::WellKnownDirectory::Work)
                    .to_string_lossy()
                    .to_string();
                (fallback.clone(), format!("{}/workspace", fallback), format!("{}/temp", fallback))
            });

        // Create feature manager
        let feature_manager = FeatureManager::new(&message);

        // Build Global shared state
        let global = Global {
            variables: variables.clone(),
            endpoints: message.resources.endpoints.clone(),
            file_table: message.file_table.clone(),
            environment_variables: message.environment_variables.clone(),
            job_display_name: message.job_display_name.clone(),
            job_id: message.job_id.clone(),
            plan_id: message.plan_id.clone(),
            timeline_id: message.timeline_id.clone(),
            pipeline_directory: pipeline_directory.clone(),
            workspace_directory: workspace_directory.clone(),
            temp_directory: self
                .host_context
                .get_directory(runner_common::constants::WellKnownDirectory::Temp)
                .to_string_lossy()
                .to_string(),
            prepend_path: Vec::new(),
            container_info: None,
            service_containers: Vec::new(),
            job_telemetry: Vec::new(),
            environment_url: None,
            cancel_token: cancel_token.clone(),
            feature_manager,
            write_debug: variables
                .get("ACTIONS_STEP_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        };

        // Create the root execution context
        let mut root_context = ExecutionContext::new_root(
            Arc::clone(&self.host_context),
            global,
            message.job_display_name.clone(),
        );

        // Set runner context
        self.set_runner_context(&mut root_context);

        // Initialize job via JobExtension (downloads actions, resolves containers, builds step list)
        let mut job_extension = JobExtension::new();
        if let Err(e) = job_extension.initialize_job(&mut root_context, &message).await {
            root_context.error(&format!("Job initialization failed: {:#}", e));
            root_context.complete(TaskResult::Failed, Some("Job initialization failed"));
            return Ok(root_context.result().unwrap_or(TaskResult::Failed));
        }

        root_context.info("Job initialized successfully.");

        // Run all steps
        let steps_runner = StepsRunner::new();
        if let Err(e) = steps_runner.run_async(&mut root_context).await {
            root_context.error(&format!("Steps execution failed: {:#}", e));
            if root_context.result().is_none() {
                root_context.complete(TaskResult::Failed, Some("Steps execution failed"));
            }
        }

        // Finalize the job (cleanup)
        job_extension.finalize_job(&mut root_context);

        // Determine final result
        let final_result = root_context.result().unwrap_or(TaskResult::Succeeded);

        trace.info(&format!("Job completed with result: {}", final_result));

        Ok(final_result)
    }

    /// Populate the runner context with OS, architecture, name, and tool cache info.
    fn set_runner_context(&self, context: &mut ExecutionContext) {
        let os = VarUtil::os().to_string();
        let arch = VarUtil::os_architecture().to_string();

        let tool_cache = self
            .host_context
            .get_directory(runner_common::constants::WellKnownDirectory::Tools)
            .to_string_lossy()
            .to_string();

        let temp = self
            .host_context
            .get_directory(runner_common::constants::WellKnownDirectory::Temp)
            .to_string_lossy()
            .to_string();

        let runner_name = context
            .global()
            .variables
            .get("system.runner.name")
            .unwrap_or_else(|| "Hosted Agent".to_string());

        let debug = context.global().write_debug;

        let runner_ctx = crate::runner_context::RunnerContext {
            os,
            arch,
            name: runner_name,
            tool_cache,
            temp,
            debug: if debug { "1".to_string() } else { String::new() },
            workspace: context.global().workspace_directory.clone(),
            environment: std::env::var("RUNNER_ENVIRONMENT").unwrap_or_else(|_| "self-hosted".to_string()),
        };

        context.set_runner_context(runner_ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_runner_new() {
        let host = HostContext::new("Worker");
        let runner = JobRunner::new(host);
        // Just verify construction doesn't panic
        let _ = runner;
    }
}
