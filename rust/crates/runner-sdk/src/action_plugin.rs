use crate::trace::TraceWriter;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Context provided to an action plugin during execution.
///
/// Maps `RunnerActionPluginExecutionContext` from `ActionPlugin.cs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPluginContext {
    /// Input key-value pairs for the plugin.
    pub inputs: HashMap<String, String>,

    /// Variables available to the plugin (e.g., `ACTIONS_STEP_DEBUG`).
    pub variables: HashMap<String, String>,

    /// Service endpoints available to the plugin.
    pub endpoints: Vec<ServiceEndpoint>,

    /// Additional context data.
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

/// A service endpoint (connection) available to a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    /// The name of the endpoint (e.g., "SystemVssConnection").
    pub name: String,

    /// The URL of the endpoint.
    pub url: String,

    /// Authorization parameters.
    pub authorization: Option<EndpointAuthorization>,
}

/// Authorization information for a service endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointAuthorization {
    /// The authorization scheme (e.g., "OAuth").
    pub scheme: String,

    /// Authorization parameters (e.g., access token).
    pub parameters: HashMap<String, String>,
}

impl ActionPluginContext {
    /// Create a new empty `ActionPluginContext`.
    pub fn new() -> Self {
        Self {
            inputs: HashMap::new(),
            variables: HashMap::new(),
            endpoints: Vec::new(),
            context: HashMap::new(),
        }
    }

    /// Get an input value by name.
    ///
    /// If `required` is true and the input is missing or empty, returns an error.
    pub fn get_input(&self, name: &str, required: bool) -> anyhow::Result<Option<String>> {
        // Case-insensitive lookup (C# uses StringComparer.OrdinalIgnoreCase)
        let value = self
            .inputs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone());

        if required {
            match &value {
                None => anyhow::bail!("Input required and not supplied: {name}"),
                Some(v) if v.is_empty() => {
                    anyhow::bail!("Input required and not supplied: {name}")
                }
                _ => {}
            }
        }

        Ok(value)
    }

    /// Get a variable value by name (case-insensitive).
    pub fn get_variable(&self, name: &str) -> Option<&String> {
        self.variables
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    /// Check if step debug is enabled via `ACTIONS_STEP_DEBUG` variable.
    pub fn is_debug(&self) -> bool {
        self.get_variable("ACTIONS_STEP_DEBUG")
            .and_then(|v| crate::string_util::StringUtil::convert_to_bool(v))
            .unwrap_or(false)
    }

    /// Get the runner context value for a given key.
    pub fn get_runner_context(&self, key: &str) -> Option<String> {
        self.context
            .get("runner")
            .and_then(|v| v.as_object())
            .and_then(|obj| obj.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Get the github context value for a given key.
    pub fn get_github_context(&self, key: &str) -> Option<String> {
        self.context
            .get("github")
            .and_then(|v| v.as_object())
            .and_then(|obj| obj.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }
}

impl Default for ActionPluginContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for action plugins.
///
/// Maps `IRunnerActionPlugin` from `ActionPlugin.cs`.
#[async_trait]
pub trait ActionPlugin: Send + Sync {
    /// Execute the plugin with the given context and trace writer.
    ///
    /// The `cancellation_token` is modelled by the caller using `tokio::select!`
    /// or `tokio_util::sync::CancellationToken`; the plugin should be written
    /// to be cancel-safe or check for cancellation cooperatively.
    async fn run(
        &self,
        context: &mut ActionPluginContext,
        trace: &dyn TraceWriter,
    ) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_input_found() {
        let mut ctx = ActionPluginContext::new();
        ctx.inputs
            .insert("myInput".to_string(), "myValue".to_string());
        let val = ctx.get_input("myinput", false).unwrap();
        assert_eq!(val, Some("myValue".to_string()));
    }

    #[test]
    fn get_input_required_missing() {
        let ctx = ActionPluginContext::new();
        let result = ctx.get_input("missing", true);
        assert!(result.is_err());
    }

    #[test]
    fn get_input_required_empty() {
        let mut ctx = ActionPluginContext::new();
        ctx.inputs.insert("key".to_string(), String::new());
        let result = ctx.get_input("key", true);
        assert!(result.is_err());
    }

    #[test]
    fn is_debug_true() {
        let mut ctx = ActionPluginContext::new();
        ctx.variables
            .insert("ACTIONS_STEP_DEBUG".to_string(), "true".to_string());
        assert!(ctx.is_debug());
    }

    #[test]
    fn is_debug_false_by_default() {
        let ctx = ActionPluginContext::new();
        assert!(!ctx.is_debug());
    }

    #[test]
    fn get_runner_context() {
        let mut ctx = ActionPluginContext::new();
        let runner_obj = serde_json::json!({
            "os": "Linux",
            "arch": "X64"
        });
        ctx.context.insert("runner".to_string(), runner_obj);
        assert_eq!(
            ctx.get_runner_context("os"),
            Some("Linux".to_string())
        );
        assert_eq!(ctx.get_runner_context("missing"), None);
    }

    #[test]
    fn serialization_roundtrip() {
        let mut ctx = ActionPluginContext::new();
        ctx.inputs
            .insert("key".to_string(), "value".to_string());
        ctx.endpoints.push(ServiceEndpoint {
            name: "SystemVssConnection".to_string(),
            url: "https://example.com".to_string(),
            authorization: Some(EndpointAuthorization {
                scheme: "OAuth".to_string(),
                parameters: {
                    let mut p = HashMap::new();
                    p.insert("AccessToken".to_string(), "token123".to_string());
                    p
                },
            }),
        });
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: ActionPluginContext = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.inputs.get("key"),
            Some(&"value".to_string())
        );
        assert_eq!(deserialized.endpoints.len(), 1);
    }
}
