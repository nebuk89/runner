// Handler trait and HandlerFactory mapping `Handler.cs`.
// Defines the interface for step execution handlers and a factory to create them.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::execution_context::ExecutionContext;
use crate::worker::ActionReference;

/// Data shared by all handler types.
#[derive(Debug, Clone)]
pub struct HandlerData {
    /// Input values for the step (with/inputs).
    pub inputs: HashMap<String, String>,

    /// Environment variables for the step.
    pub environment: HashMap<String, String>,

    /// The action context (reference info, resolved paths).
    pub action_context: ActionContext,
}

/// Context about the resolved action being executed.
#[derive(Debug, Clone, Default)]
pub struct ActionContext {
    /// The action reference (e.g. "actions/checkout@v4").
    pub reference: Option<ActionReference>,

    /// Resolved local path to the action directory.
    pub action_directory: String,

    /// The entry point file (e.g. "dist/index.js", "entrypoint.sh").
    pub entry_point: String,

    /// The type of action: "node", "docker", "composite", "script".
    pub action_type: String,

    /// Pre-step entry point (for pre: in action.yml).
    pub pre_entry_point: Option<String>,

    /// Post-step entry point (for post: in action.yml).
    pub post_entry_point: Option<String>,

    /// Plugin-provided image for container actions.
    pub image: Option<String>,

    /// Docker file path for container actions that build.
    pub dockerfile: Option<String>,
}

/// Trait for step execution handlers.
#[async_trait]
pub trait Handler: Send + Sync {
    /// Execute the step.
    async fn run_async(
        &self,
        context: &mut ExecutionContext,
        data: &HandlerData,
    ) -> anyhow::Result<()>;

    /// Prepare the execution environment (set up env vars, print info).
    fn prepare_execution(
        &self,
        context: &mut ExecutionContext,
        data: &HandlerData,
    ) {
        // Print action details
        context.section(&format!(
            "Run {}",
            data.action_context
                .reference
                .as_ref()
                .map(|r| r.name.as_str())
                .unwrap_or(context.display_name())
        ));

        // Inject INPUT_* environment variables
        for (key, value) in &data.inputs {
            let env_name = format!("INPUT_{}", key.to_uppercase().replace(' ', "_"));
            context
                .step_environment
                .insert(env_name, value.clone());
        }

        // Merge handler-level environment
        for (key, value) in &data.environment {
            context.step_environment.insert(key.clone(), value.clone());
        }
    }
}

/// Factory for creating the appropriate handler for a step.
pub struct HandlerFactory;

impl HandlerFactory {
    /// Create a handler for the given action type.
    pub fn create(action_type: &str) -> Box<dyn Handler> {
        match action_type {
            "node" | "node12" | "node16" | "node20" | "node24" => {
                Box::new(super::node_script_handler::NodeScriptActionHandler::new())
            }
            "docker" | "container" => {
                Box::new(super::container_handler::ContainerActionHandler::new())
            }
            "composite" => {
                Box::new(super::composite_handler::CompositeActionHandler::new())
            }
            "script" | "run" | _ => {
                Box::new(super::script_handler::ScriptHandler::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handler_factory_script() {
        let handler = HandlerFactory::create("script");
        // Just ensure it creates something
        let _ = handler;
    }

    #[test]
    fn test_handler_factory_node() {
        let handler = HandlerFactory::create("node20");
        let _ = handler;
    }

    #[test]
    fn test_handler_factory_composite() {
        let handler = HandlerFactory::create("composite");
        let _ = handler;
    }

    #[test]
    fn test_handler_factory_docker() {
        let handler = HandlerFactory::create("docker");
        let _ = handler;
    }

    #[test]
    fn test_handler_data_default() {
        let data = HandlerData {
            inputs: HashMap::new(),
            environment: HashMap::new(),
            action_context: ActionContext::default(),
        };
        assert!(data.inputs.is_empty());
    }
}
