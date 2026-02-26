// CompositeActionHandler mapping `CompositeActionHandler.cs`.
// Handles composite actions by expanding their steps and enqueueing them
// into the parent execution context for the StepsRunner to execute.

use async_trait::async_trait;
use anyhow::{Context, Result};
use std::collections::HashMap;

use runner_common::constants;

use crate::execution_context::{ExecutionContext, IStep};
use crate::handlers::handler::{ActionContext, Handler, HandlerData};
use crate::action_manifest_manager::{ActionDefinition, ActionStepDefinition};

/// Handler for composite actions (action.yml with `using: composite`).
pub struct CompositeActionHandler;

impl CompositeActionHandler {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Handler for CompositeActionHandler {
    async fn run_async(
        &self,
        context: &mut ExecutionContext,
        data: &HandlerData,
    ) -> Result<()> {
        self.prepare_execution(context, data);

        // Check recursion depth
        if context.depth() >= constants::COMPOSITE_ACTIONS_MAX_DEPTH {
            context.error(&format!(
                "Composite action depth exceeded maximum of {}.",
                constants::COMPOSITE_ACTIONS_MAX_DEPTH
            ));
            context.complete(
                runner_common::util::task_result_util::TaskResult::Failed,
                Some("Maximum composite action depth exceeded"),
            );
            return Ok(());
        }

        // Load the action definition
        let action_dir = &data.action_context.action_directory;
        let definition = crate::action_manifest_manager::ActionManifestManager::load_action(action_dir)?;

        context.info(&format!(
            "Composite action '{}' has {} steps.",
            definition.name,
            definition.steps.len()
        ));

        // Create a child context for the composite action's steps
        let mut child_context = context.create_child(format!("Composite: {}", definition.name));

        // Map composite inputs to environment variables
        let mut composite_env = HashMap::new();
        for (input_name, default_value) in &definition.inputs {
            let value = data
                .inputs
                .get(input_name)
                .cloned()
                .unwrap_or_else(|| default_value.clone());

            composite_env.insert(
                format!("INPUT_{}", input_name.to_uppercase().replace(' ', "_")),
                value,
            );
        }

        // Enqueue composite steps
        for (i, step_def) in definition.steps.iter().enumerate() {
            let step = CompositeStep {
                id: step_def.id.clone().unwrap_or_else(|| format!("__composite_{}_{}", context.depth(), i)),
                display_name: step_def.name.clone().unwrap_or_else(|| format!("Step {}", i + 1)),
                condition: step_def.condition.clone().unwrap_or_default(),
                timeout: step_def.timeout_in_minutes.unwrap_or(0),
                continue_on_error: step_def.continue_on_error.unwrap_or(false),
                step_definition: step_def.clone(),
                composite_env: composite_env.clone(),
                action_directory: action_dir.clone(),
            };

            child_context.job_steps.push_back(Box::new(step));
        }

        // Run the child context's steps using a nested StepsRunner
        let steps_runner = crate::steps_runner::StepsRunner::new();
        steps_runner.run_async(&mut child_context).await?;

        // Propagate outputs from child to parent
        for (key, value) in &child_context.outputs {
            // Only propagate declared outputs
            if definition.outputs.contains_key(key) {
                context.outputs.insert(key.clone(), value.clone());
            }
        }

        // Propagate result
        if let Some(result) = child_context.result() {
            context.set_result(result);
        }

        context.end_section();

        Ok(())
    }
}

/// A single step within a composite action.
struct CompositeStep {
    id: String,
    display_name: String,
    condition: String,
    timeout: u32,
    continue_on_error: bool,
    step_definition: ActionStepDefinition,
    composite_env: HashMap<String, String>,
    action_directory: String,
}

impl IStep for CompositeStep {
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
        match self.step_definition.uses.as_deref() {
            Some(_) => "action",
            None => "script",
        }
    }

    fn run_async<'a>(
        &'a self,
        context: &'a mut ExecutionContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Inject composite environment
            for (k, v) in &self.composite_env {
                context.step_environment.insert(k.clone(), v.clone());
            }

            // Add step-level environment
            if let Some(ref env) = self.step_definition.env {
                for (k, v) in env {
                    context.step_environment.insert(k.clone(), v.clone());
                }
            }

            if let Some(ref uses) = self.step_definition.uses {
                // This is a nested action reference
                context.info(&format!("Uses: {}", uses));

                // For nested actions, we would need to resolve and run them
                // via the action manager. For now, log it.
                context.warning("Nested action references in composite actions require full action resolution.");
                Ok(())
            } else if let Some(ref run) = self.step_definition.run {
                // This is an inline script step
                let shell = self
                    .step_definition
                    .shell
                    .clone()
                    .unwrap_or_else(|| crate::handlers::script_handler::ScriptHandlerHelpers::get_default_shell());

                let mut inputs = HashMap::new();
                inputs.insert("script".to_string(), run.clone());
                inputs.insert("shell".to_string(), shell);

                if let Some(ref wd) = self.step_definition.working_directory {
                    inputs.insert("working-directory".to_string(), wd.clone());
                }

                let handler_data = HandlerData {
                    inputs,
                    environment: self.composite_env.clone(),
                    action_context: ActionContext {
                        action_type: "script".to_string(),
                        action_directory: self.action_directory.clone(),
                        ..ActionContext::default()
                    },
                };

                let handler = crate::handlers::script_handler::ScriptHandler::new();
                handler.run_async(context, &handler_data).await
            } else {
                context.warning("Composite step has neither 'uses' nor 'run'.");
                Ok(())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_composite_handler_creation() {
        let handler = CompositeActionHandler::new();
        let _ = handler;
    }
}
