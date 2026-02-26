// NodeScriptActionHandler mapping `NodeScriptActionHandler.cs`.
// Executes JavaScript/TypeScript actions by invoking the appropriate Node.js binary.

use async_trait::async_trait;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

use runner_common::constants;
use runner_common::host_context::HostContext;
use runner_common::util::node_util::NodeUtil;

use crate::execution_context::ExecutionContext;
use crate::handlers::handler::{Handler, HandlerData};
use crate::handlers::step_host::{DefaultStepHost, StepHost};

/// Handler for Node.js-based actions (node12, node16, node20, node24).
pub struct NodeScriptActionHandler;

impl NodeScriptActionHandler {
    pub fn new() -> Self {
        Self
    }

    /// Resolve the path to the Node.js binary for the given action.
    fn resolve_node_binary(
        &self,
        context: &ExecutionContext,
        data: &HandlerData,
    ) -> Result<(PathBuf, Option<String>)> {
        // Determine which Node version the action wants
        let action_node_version = data
            .action_context
            .action_type
            .strip_prefix("node")
            .unwrap_or("20");

        // Check feature flags for node migration
        let global = context.global();
        let use_node24_by_default = global
            .feature_manager
            .is_feature_enabled(constants::node_migration::USE_NODE24_BY_DEFAULT_FLAG);
        let require_node24 = global
            .feature_manager
            .is_feature_enabled(constants::node_migration::REQUIRE_NODE24_FLAG);
        drop(global);

        let workflow_env = Some(context.global().environment_variables.clone());

        let (node_version, warning) = NodeUtil::determine_actions_node_version(
            workflow_env.as_ref(),
            use_node24_by_default,
            require_node24,
        );

        // Resolve the binary path in the externals directory
        let externals_dir = context
            .host_context()
            .get_directory(runner_common::constants::WellKnownDirectory::Externals);

        let node_dir = externals_dir.join(&node_version).join("bin");

        let node_binary = if cfg!(windows) {
            node_dir.join("node.exe")
        } else {
            node_dir.join("node")
        };

        Ok((node_binary, warning))
    }
}

#[async_trait]
impl Handler for NodeScriptActionHandler {
    async fn run_async(
        &self,
        context: &mut ExecutionContext,
        data: &HandlerData,
    ) -> Result<()> {
        self.prepare_execution(context, data);

        // Resolve Node.js binary
        let (node_binary, warning) = self.resolve_node_binary(context, data)?;

        // Emit warning about node version if applicable
        if let Some(ref warn_msg) = warning {
            context.warning(warn_msg);
        }

        let node_binary_str = node_binary.to_string_lossy().to_string();
        context.debug(&format!("Node binary: {}", node_binary_str));

        // Build the script path
        let action_dir = &data.action_context.action_directory;
        let entry_point = &data.action_context.entry_point;
        let script_path = if entry_point.is_empty() {
            format!("{}/dist/index.js", action_dir)
        } else {
            format!("{}/{}", action_dir, entry_point)
        };

        context.debug(&format!("Script: {}", script_path));

        // Build environment
        let mut env = context.global().environment_variables.clone();
        for (k, v) in &context.step_environment {
            env.insert(k.clone(), v.clone());
        }

        // Inject INPUT_* environment variables
        for (key, value) in &data.inputs {
            let env_name = format!("INPUT_{}", key.to_uppercase().replace(' ', "_"));
            env.insert(env_name, value.clone());
        }

        // Inject actions runtime environment variables
        self.inject_runtime_env(context, &mut env);

        // Prepend paths
        let prepend = context.global().prepend_path.clone();
        if !prepend.is_empty() {
            let current_path = env
                .get(runner_common::constants::PATH_VARIABLE)
                .cloned()
                .or_else(|| std::env::var(runner_common::constants::PATH_VARIABLE).ok())
                .unwrap_or_default();

            let separator = if cfg!(windows) { ";" } else { ":" };
            let new_path = format!("{}{}{}", prepend.join(separator), separator, current_path);
            env.insert(
                runner_common::constants::PATH_VARIABLE.to_string(),
                new_path,
            );
        }

        // Working directory
        let working_directory = data
            .inputs
            .get("working-directory")
            .cloned()
            .unwrap_or_else(|| context.global().workspace_directory.clone());

        // Execute
        let step_host = DefaultStepHost::new();
        let exit_code = step_host
            .execute_async(
                &working_directory,
                &node_binary_str,
                &script_path,
                &env,
                context.cancel_token(),
            )
            .await?;

        if exit_code != 0 {
            context.error(&format!(
                "Node.js action completed with exit code {}.",
                exit_code
            ));
            context.complete(
                runner_common::util::task_result_util::TaskResult::Failed,
                Some(&format!("Exit code {}", exit_code)),
            );
        }

        context.end_section();

        Ok(())
    }
}

impl NodeScriptActionHandler {
    /// Inject runtime environment variables needed by JavaScript actions.
    fn inject_runtime_env(
        &self,
        context: &ExecutionContext,
        env: &mut HashMap<String, String>,
    ) {
        let global = context.global();

        // GITHUB_ACTION - the action name/ref
        if let Some(step_id) = context.current_step_id() {
            env.insert("GITHUB_ACTION".to_string(), step_id.to_string());
        }

        // GITHUB_ACTION_PATH
        env.insert(
            "GITHUB_ACTION_PATH".to_string(),
            String::new(), // Will be set by the action manager
        );

        // Find the SystemConnection endpoint for runtime URL/token
        for endpoint in &global.endpoints {
            if endpoint.name == "SystemVssConnection" {
                env.insert(
                    "ACTIONS_RUNTIME_URL".to_string(),
                    endpoint.url.clone(),
                );

                if let Some(ref auth) = endpoint.authorization {
                    if let Some(token) = auth.parameters.get("AccessToken") {
                        env.insert("ACTIONS_RUNTIME_TOKEN".to_string(), token.clone());
                    }
                }
                break;
            }
        }

        // ACTIONS_CACHE_URL
        for endpoint in &global.endpoints {
            if endpoint.name == "RuntimeCache" {
                env.insert("ACTIONS_CACHE_URL".to_string(), endpoint.url.clone());
                break;
            }
        }

        // ACTIONS_ID_TOKEN_REQUEST_URL and TOKEN
        for endpoint in &global.endpoints {
            if endpoint.name == "ActionsIdTokenRequest" {
                env.insert(
                    "ACTIONS_ID_TOKEN_REQUEST_URL".to_string(),
                    endpoint.url.clone(),
                );
                if let Some(ref auth) = endpoint.authorization {
                    if let Some(token) = auth.parameters.get("AccessToken") {
                        env.insert(
                            "ACTIONS_ID_TOKEN_REQUEST_TOKEN".to_string(),
                            token.clone(),
                        );
                    }
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handler_creation() {
        let handler = NodeScriptActionHandler::new();
        let _ = handler;
    }
}
