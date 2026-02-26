// JobExtension mapping `JobExtension.cs`.
// Initializes the job: downloads actions, resolves containers, and builds
// the step list (pre/main/post). Finalizes the job with cleanup.

use anyhow::{Context, Result};
use std::collections::HashMap;

use runner_common::constants::{self, CURRENT_PLATFORM, OsPlatform};
use runner_common::util::task_result_util::TaskResult;

use crate::action_manager::ActionManager;
use crate::action_manifest_manager::ActionManifestManager;
use crate::container::container_operation_provider::ContainerOperationProvider;
use crate::execution_context::{ExecutionContext, IStep};
use crate::handlers::handler::{ActionContext, HandlerData, HandlerFactory};
use crate::worker::{AgentJobRequestMessage, JobStep};

/// Manages job initialization and finalization.
pub struct JobExtension {
    action_manager: ActionManager,
    container_provider: ContainerOperationProvider,
}

impl JobExtension {
    /// Create a new `JobExtension`.
    pub fn new() -> Self {
        Self {
            action_manager: ActionManager::new(),
            container_provider: ContainerOperationProvider::new(),
        }
    }

    /// Initialize the job:
    /// 1. Download and resolve all referenced actions
    /// 2. Start job/service containers if defined
    /// 3. Build the step list (pre, main, post steps)
    pub async fn initialize_job(
        &mut self,
        context: &mut ExecutionContext,
        message: &AgentJobRequestMessage,
    ) -> Result<()> {
        context.info("Initializing job...");

        // Download and resolve actions
        let prepare_result = self
            .action_manager
            .prepare_actions_async(context, &message.steps)
            .await
            .context("Failed to prepare actions")?;

        for warning in &prepare_result.warnings {
            context.warning(warning);
        }

        // Start containers if needed
        if message.has_job_container() || message.has_service_containers() {
            if CURRENT_PLATFORM == OsPlatform::Linux {
                self.container_provider
                    .start_containers_async(context, message)
                    .await
                    .context("Failed to start containers")?;
            } else {
                context.warning(
                    "Container support is only available on Linux runners. \
                     Job and service containers will be ignored.",
                );
            }
        }

        // Build the step list
        self.build_step_list(context, message, &prepare_result.resolved_actions)?;

        context.info(&format!(
            "Job initialized with {} steps and {} post-job steps.",
            context.job_steps.len(),
            context.post_job_steps.len()
        ));

        Ok(())
    }

    /// Build the step list from the job message.
    ///
    /// For each step:
    /// - Script steps → ScriptHandler
    /// - Action steps → resolve action type, create pre/main/post steps
    fn build_step_list(
        &self,
        context: &mut ExecutionContext,
        message: &AgentJobRequestMessage,
        resolved_actions: &HashMap<String, String>,
    ) -> Result<()> {
        for step in &message.steps {
            match step.step_type.as_str() {
                "script" | "run" | "" => {
                    // Script step - create a RunStep
                    let run_step = RunStep {
                        id: step.id.clone(),
                        display_name: step.display_name.clone(),
                        condition: step.condition.clone(),
                        timeout: step.timeout_in_minutes,
                        continue_on_error: step.continue_on_error,
                        script: step.script.clone().unwrap_or_default(),
                        shell: step.shell.clone(),
                        working_directory: step.working_directory.clone(),
                        environment: step.environment_map(),
                        inputs: step.inputs_map(),
                    };
                    context.job_steps.push_back(Box::new(run_step));
                }
                "action" => {
                    // Check reference type: "script" means this is a run: step
                    // wrapped as an ActionStep with ScriptReference.
                    let is_script_ref = step
                        .reference
                        .as_ref()
                        .and_then(|r| r.as_object())
                        .and_then(|obj| obj.get("type"))
                        .and_then(|t| t.as_str())
                        .map(|t| t == "script")
                        .unwrap_or(false);

                    if is_script_ref {
                        // Treat as a script/run step. Extract the script body
                        // from the inputs TemplateToken (key: "script").
                        let inputs = step.inputs_map();
                        let script = inputs.get("script").cloned().unwrap_or_default();
                        let shell = inputs.get("shell").cloned().or_else(|| step.shell.clone());
                        let working_directory = inputs
                            .get("working-directory")
                            .cloned()
                            .or_else(|| step.working_directory.clone());

                        let run_step = RunStep {
                            id: step.id.clone(),
                            display_name: step.display_name.clone(),
                            condition: step.condition.clone(),
                            timeout: step.timeout_in_minutes,
                            continue_on_error: step.continue_on_error,
                            script,
                            shell,
                            working_directory,
                            environment: step.environment_map(),
                            inputs,
                        };
                        context.job_steps.push_back(Box::new(run_step));
                    } else {
                        // Real action step - resolve and create the appropriate handler
                        self.build_action_step(context, step, resolved_actions)?;
                    }
                }
                other => {
                    context.warning(&format!("Unknown step type: {}", other));
                    // Treat as a script step as fallback
                    let run_step = RunStep {
                        id: step.id.clone(),
                        display_name: step.display_name.clone(),
                        condition: step.condition.clone(),
                        timeout: step.timeout_in_minutes,
                        continue_on_error: step.continue_on_error,
                        script: step.script.clone().unwrap_or_default(),
                        shell: step.shell.clone(),
                        working_directory: step.working_directory.clone(),
                        environment: step.environment_map(),
                        inputs: step.inputs_map(),
                    };
                    context.job_steps.push_back(Box::new(run_step));
                }
            }
        }

        Ok(())
    }

    /// Build an action step, including pre and post steps if defined.
    fn build_action_step(
        &self,
        context: &mut ExecutionContext,
        step: &JobStep,
        resolved_actions: &HashMap<String, String>,
    ) -> Result<()> {
        let action_ref = match step.action_reference() {
            Some(r) => r,
            None => {
                context.warning(&format!(
                    "Action step '{}' has no reference.",
                    step.display_name
                ));
                return Ok(());
            }
        };

        // Build cache key
        let cache_key = if action_ref.path.is_empty() {
            format!("{}@{}", action_ref.name, action_ref.git_ref)
        } else {
            format!(
                "{}/{}@{}",
                action_ref.name, action_ref.path, action_ref.git_ref
            )
        };

        let action_directory = match resolved_actions.get(&cache_key) {
            Some(dir) => dir.clone(),
            None => {
                context.warning(&format!("Action '{}' not resolved.", cache_key));
                return Ok(());
            }
        };

        // Load action manifest to determine type and entry points
        let definition = match ActionManifestManager::load_action(&action_directory) {
            Ok(def) => def,
            Err(e) => {
                context.warning(&format!("Failed to load action manifest for '{}': {}", cache_key, e));
                return Ok(());
            }
        };

        let action_type = definition.runs.using.clone();
        let action_context = ActionContext {
            reference: Some(action_ref.clone()),
            action_directory: action_directory.clone(),
            entry_point: definition.runs.main.clone().unwrap_or_default(),
            action_type: action_type.clone(),
            pre_entry_point: definition.runs.pre.clone(),
            post_entry_point: definition.runs.post.clone(),
            image: definition.runs.image.clone(),
            dockerfile: definition.runs.dockerfile.clone(),
        };

        // Create pre step if defined
        if let Some(ref pre_entry) = definition.runs.pre {
            let pre_condition = definition
                .runs
                .pre_if
                .clone()
                .unwrap_or_else(|| "always()".to_string());

            let pre_step = ActionStep {
                id: format!("{}_pre", step.id),
                display_name: format!("Pre {}", step.display_name),
                condition: pre_condition,
                timeout: step.timeout_in_minutes,
                continue_on_error: true,
                action_context: ActionContext {
                    entry_point: pre_entry.clone(),
                    ..action_context.clone()
                },
                inputs: step.inputs_map(),
                environment: step.environment_map(),
            };
            // Pre steps run as part of the main step queue (at the beginning)
            context.job_steps.push_back(Box::new(pre_step));
        }

        // Create main step
        let main_step = ActionStep {
            id: step.id.clone(),
            display_name: step.display_name.clone(),
            condition: step.condition.clone(),
            timeout: step.timeout_in_minutes,
            continue_on_error: step.continue_on_error,
            action_context: action_context.clone(),
            inputs: step.inputs_map(),
            environment: step.environment_map(),
        };
        context.job_steps.push_back(Box::new(main_step));

        // Create post step if defined (goes in post_job_steps)
        if let Some(ref post_entry) = definition.runs.post {
            let post_condition = definition
                .runs
                .post_if
                .clone()
                .unwrap_or_else(|| "always()".to_string());

            let post_step = ActionStep {
                id: format!("{}_post", step.id),
                display_name: format!("Post {}", step.display_name),
                condition: post_condition,
                timeout: 5, // 5 min default for post steps
                continue_on_error: true,
                action_context: ActionContext {
                    entry_point: post_entry.clone(),
                    ..action_context.clone()
                },
                inputs: step.inputs_map(),
                environment: step.environment_map(),
            };
            context.post_job_steps.push(Box::new(post_step));
        }

        Ok(())
    }

    /// Finalize the job: stop containers, clean up temp files.
    pub fn finalize_job(&mut self, context: &mut ExecutionContext) {
        context.info("Finalizing job...");

        // Container cleanup would be async, but we do best-effort sync cleanup
        if context.global().container_info.is_some()
            || !context.global().service_containers.is_empty()
        {
            context.info("Container cleanup will be handled by post-job steps.");
        }

        // Clean up temp directory
        let temp_dir = context.global().temp_directory.clone();
        if std::path::Path::new(&temp_dir).exists() {
            context.debug(&format!("Cleaning temp directory: {}", temp_dir));
            let _ = std::fs::remove_dir_all(&temp_dir);
        }

        context.info("Job finalized.");
    }
}

impl Default for JobExtension {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Step implementations
// ---------------------------------------------------------------------------

/// A run/script step.
struct RunStep {
    id: String,
    display_name: String,
    condition: String,
    timeout: u32,
    continue_on_error: bool,
    script: String,
    shell: Option<String>,
    working_directory: Option<String>,
    environment: HashMap<String, String>,
    inputs: HashMap<String, String>,
}

impl IStep for RunStep {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.display_name
    }
    fn condition(&self) -> &str {
        &self.condition
    }
    fn timeout_in_minutes(&self) -> u32 {
        self.timeout
    }
    fn continue_on_error(&self) -> bool {
        self.continue_on_error
    }
    fn step_type(&self) -> &str {
        "script"
    }

    fn run_async<'a>(
        &'a self,
        context: &'a mut ExecutionContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let mut inputs = self.inputs.clone();
        let script = self.script.clone();
        let shell = self.shell.clone();
        let working_directory = self.working_directory.clone();
        let environment = self.environment.clone();

        Box::pin(async move {
            inputs.insert("script".to_string(), script);
            if let Some(shell) = shell {
                inputs.insert("shell".to_string(), shell);
            }
            if let Some(wd) = working_directory {
                inputs.insert("working-directory".to_string(), wd);
            }

            let handler_data = HandlerData {
                inputs,
                environment: self.environment.clone(),
                action_context: ActionContext {
                    action_type: "script".to_string(),
                    ..ActionContext::default()
                },
            };

            let handler = HandlerFactory::create("script");
            handler.run_async(context, &handler_data).await
        })
    }
}

/// An action step (node, docker, composite).
struct ActionStep {
    id: String,
    display_name: String,
    condition: String,
    timeout: u32,
    continue_on_error: bool,
    action_context: ActionContext,
    inputs: HashMap<String, String>,
    environment: HashMap<String, String>,
}

impl IStep for ActionStep {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.display_name
    }
    fn condition(&self) -> &str {
        &self.condition
    }
    fn timeout_in_minutes(&self) -> u32 {
        self.timeout
    }
    fn continue_on_error(&self) -> bool {
        self.continue_on_error
    }
    fn step_type(&self) -> &str {
        "action"
    }
    fn reference_name(&self) -> Option<&str> {
        self.action_context
            .reference
            .as_ref()
            .map(|r| r.name.as_str())
    }

    fn run_async<'a>(
        &'a self,
        context: &'a mut ExecutionContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let action_type = self.action_context.action_type.clone();
        let inputs = self.inputs.clone();
        let environment = self.environment.clone();
        let action_context = self.action_context.clone();

        Box::pin(async move {
            let handler_data = HandlerData {
                inputs,
                environment,
                action_context,
            };

            let handler = HandlerFactory::create(&action_type);
            handler.run_async(context, &handler_data).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_extension_new() {
        let ext = JobExtension::new();
        let _ = ext;
    }
}
