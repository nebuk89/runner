// Worker mapping `Worker.cs`.
// The top-level service that receives a job message from IPC, starts the job runner,
// and listens for cancellation messages concurrently.

use anyhow::{Context, Result};
use runner_common::host_context::HostContext;
use runner_common::process_channel::{MessageType, ProcessChannel};
use runner_common::secret_masker::SecretMasker;
use runner_common::util::task_result_util::TaskResult;
use runner_sdk::TraceWriter;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::job_runner::JobRunner;
use crate::run_server::RunServer;

/// Deserialized job request message from the listener.
/// Maps `Pipelines.AgentJobRequestMessage` from the C# runner.
///
/// The C# server uses `CamelCasePropertyNamesContractResolver` which serializes
/// all property names as camelCase. Complex types like TemplateToken, Plan, and
/// Timeline are nested objects. Fields we don't fully model yet are captured as
/// `serde_json::Value` so deserialization never fails.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentJobRequestMessage {
    /// Unique ID for this job.
    #[serde(default)]
    pub job_id: String,

    /// Display name of the job.
    #[serde(default)]
    pub job_display_name: String,

    /// The request ID assigned by the server.
    #[serde(default)]
    pub request_id: u64,

    /// Plan reference (nested object with planId, scopeIdentifier, etc.).
    #[serde(default)]
    pub plan: Option<PlanReference>,

    /// Timeline reference (nested object with id, changeId, location).
    #[serde(default)]
    pub timeline: Option<TimelineReference>,

    /// Job-level environment variables.
    /// In C# this is `List<TemplateToken>` — a list of mapping tokens.
    /// We deserialize as raw JSON and convert to a flat HashMap later.
    #[serde(default)]
    pub environment_variables: Vec<serde_json::Value>,

    /// Variables (name → VariableValueMessage).
    #[serde(default)]
    pub variables: std::collections::HashMap<String, VariableValueMessage>,

    /// Steps to execute.
    #[serde(default)]
    pub steps: Vec<JobStep>,

    /// Service endpoints for connecting back to the server.
    #[serde(default)]
    pub resources: JobResources,

    /// Workspace / repository information.
    #[serde(default)]
    pub workspace: Option<serde_json::Value>,

    /// File table entries — just file paths (strings).
    #[serde(default)]
    pub file_table: Vec<String>,

    /// Context data (github, runner, needs, strategy, matrix, inputs).
    #[serde(default)]
    pub context_data: std::collections::HashMap<String, serde_json::Value>,

    /// Job container definition (TemplateToken — complex nested structure).
    #[serde(default)]
    pub job_container: Option<serde_json::Value>,

    /// Service containers (TemplateToken — complex nested structure).
    #[serde(default)]
    pub job_service_containers: Option<serde_json::Value>,

    /// Actor requesting the workflow.
    #[serde(default)]
    pub actor: String,

    /// Message type discriminator.
    #[serde(default)]
    pub message_type: String,

    /// Catch-all for any extra fields we don't explicitly handle.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl AgentJobRequestMessage {
    /// Extract the plan ID from the nested plan reference.
    pub fn plan_id(&self) -> String {
        self.plan
            .as_ref()
            .map(|p| p.plan_id.clone())
            .unwrap_or_default()
    }

    /// Extract the timeline ID from the nested timeline reference.
    pub fn timeline_id(&self) -> String {
        self.timeline
            .as_ref()
            .map(|t| t.id.clone())
            .unwrap_or_default()
    }

    /// Convert the TemplateToken environment variables into a flat HashMap.
    /// TemplateTokens are complex polymorphic types from C#. Simple scalars
    /// serialize as plain JSON values; mappings use `{"type": 2, "map": [...]}`.
    pub fn environment_variables_map(&self) -> std::collections::HashMap<String, String> {
        let mut result = std::collections::HashMap::new();
        for token in &self.environment_variables {
            Self::extract_env_from_template_token(token, &mut result);
        }
        result
    }

    fn extract_env_from_template_token(
        token: &serde_json::Value,
        out: &mut std::collections::HashMap<String, String>,
    ) {
        // TemplateToken serialisation:
        // - If the token is a plain JSON object with a "map" array, it's a
        //   MappingToken where entries alternate key, value, key, value, ...
        // - Each sub-token that's a plain string/number/bool is a literal.
        // - Sub-tokens can also be objects with a "lit" field.
        if let Some(obj) = token.as_object() {
            if let Some(map_arr) = obj.get("map").and_then(|v| v.as_array()) {
                // Pairs: key, value, key, value, ...
                let mut iter = map_arr.iter();
                while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                    let key = Self::template_token_to_string(k);
                    let val = Self::template_token_to_string(v);
                    if let (Some(k), Some(v)) = (key, val) {
                        out.insert(k, v);
                    }
                }
            }
        }
    }

    fn template_token_to_string(token: &serde_json::Value) -> Option<String> {
        match token {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            serde_json::Value::Object(obj) => {
                // Object form: {"type": N, "lit": "value"} or similar
                obj.get("lit")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
            _ => None,
        }
    }

    /// Check if job containers are defined.
    pub fn has_job_container(&self) -> bool {
        self.job_container.as_ref().map_or(false, |v| !v.is_null())
    }

    /// Check if service containers are defined.
    pub fn has_service_containers(&self) -> bool {
        self.job_service_containers
            .as_ref()
            .map_or(false, |v| !v.is_null())
    }
}

/// Variable value from the job message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableValueMessage {
    pub value: String,
    #[serde(default)]
    pub is_secret: bool,
    #[serde(default)]
    pub is_read_only: bool,
}

/// A single step definition from the job message.
/// C# uses polymorphic ActionStep : TaskStep : Step with type discriminators.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStep {
    /// Step ID.
    #[serde(default)]
    pub id: String,

    /// Human-friendly display name.
    #[serde(default)]
    pub display_name: String,

    /// Condition expression (e.g. "success()", "always()").
    #[serde(default)]
    pub condition: String,

    /// Timeout in minutes.
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub timeout_in_minutes: u32,

    /// Type of step: C# StepType enum serialized as string ("action").
    /// The C# JSON key is "type" (camelCased from "Type").
    #[serde(default, rename = "type")]
    pub step_type: String,

    /// Reference for action steps (e.g. "actions/checkout@v4").
    /// C# uses polymorphic ActionStepDefinitionReference.
    #[serde(default)]
    pub reference: Option<serde_json::Value>,

    /// Inline inputs / with values.
    /// C# sends this as a TemplateToken (recursive AST), not a simple key→value map.
    #[serde(default)]
    pub inputs: Option<serde_json::Value>,

    /// Step-level environment variables (TemplateToken in C#).
    #[serde(default)]
    pub environment: Option<serde_json::Value>,

    /// Continue on error flag. C# sends null when not set.
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub continue_on_error: bool,

    /// The script body for run steps (not a top-level field in C#, extracted from inputs).
    #[serde(default)]
    pub script: Option<String>,

    /// Shell override (bash, pwsh, python, etc.).
    #[serde(default)]
    pub shell: Option<String>,

    /// Working directory override.
    #[serde(default)]
    pub working_directory: Option<String>,

    /// Whether the step is enabled (C# default: true).
    #[serde(default = "default_true", deserialize_with = "deserialize_null_as_true")]
    pub enabled: bool,

    /// Context name for the step (C# ActionStep.ContextName).
    #[serde(default)]
    pub context_name: Option<String>,

    /// Catch-all for extra step fields.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// Deserialize a value that might be `null` as the type's `Default`.
/// serde `#[serde(default)]` only kicks in when the key is *absent*;
/// this handles the case where the key is present with a JSON `null`.
fn deserialize_null_as_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    let opt = Option::<T>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// Deserialize a bool that might be `null`, defaulting to `true`.
fn deserialize_null_as_true<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<bool>::deserialize(deserializer)?;
    Ok(opt.unwrap_or(true))
}

use serde::Deserialize as _;

impl JobStep {
    /// Extract the action reference as a structured type if possible.
    pub fn action_reference(&self) -> Option<ActionReference> {
        self.reference
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Extract the inputs as a flat HashMap.
    /// C# sends inputs as a TemplateToken (type=2 MappingToken with map array).
    /// Each map entry has `Key` and `Value` (PascalCase) with `lit` string values.
    pub fn inputs_map(&self) -> std::collections::HashMap<String, String> {
        let mut result = std::collections::HashMap::new();
        if let Some(ref inputs_val) = self.inputs {
            if let Some(obj) = inputs_val.as_object() {
                if let Some(map_arr) = obj.get("map").and_then(|v| v.as_array()) {
                    for entry in map_arr {
                        // Format: {"Key": {"type": 0, "lit": "name"}, "Value": {"type": 0, "lit": "value"}}
                        if let Some(entry_obj) = entry.as_object() {
                            let key = entry_obj
                                .get("Key")
                                .or_else(|| entry_obj.get("key"))
                                .and_then(|k| AgentJobRequestMessage::template_token_to_string(k));
                            let val = entry_obj
                                .get("Value")
                                .or_else(|| entry_obj.get("value"))
                                .and_then(|v| AgentJobRequestMessage::template_token_to_string(v));
                            if let (Some(k), Some(v)) = (key, val) {
                                result.insert(k, v);
                            }
                        }
                    }
                } else {
                    // Simple object mapping fallback
                    for (k, v) in obj {
                        if let Some(s) = v.as_str() {
                            result.insert(k.clone(), s.to_string());
                        }
                    }
                }
            }
        }
        result
    }

    /// Extract the environment as a flat HashMap.
    /// C# sends environment as TemplateToken or a simple mapping.
    pub fn environment_map(&self) -> std::collections::HashMap<String, String> {
        let mut result = std::collections::HashMap::new();
        if let Some(ref env_val) = self.environment {
            // If it's a simple JSON object with string values, extract directly
            if let Some(obj) = env_val.as_object() {
                // Check if it looks like a TemplateToken (has "type" and "map" fields)
                if let Some(map_arr) = obj.get("map").and_then(|v| v.as_array()) {
                    let mut iter = map_arr.iter();
                    while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                        if let (Some(key), Some(val)) = (
                            AgentJobRequestMessage::template_token_to_string(k),
                            AgentJobRequestMessage::template_token_to_string(v),
                        ) {
                            result.insert(key, val);
                        }
                    }
                } else {
                    // Simple object mapping
                    for (k, v) in obj {
                        if let Some(s) = v.as_str() {
                            result.insert(k.clone(), s.to_string());
                        }
                    }
                }
            }
        }
        result
    }
}

/// Action reference in a step.
/// C# has polymorphic ActionStepDefinitionReference with subclasses
/// RepositoryPathReference, ContainerRegistryReference, ScriptReference.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReference {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub repository_type: String,
    /// Type discriminator from C# ("repository", "containerRegistry", "script").
    #[serde(default, rename = "type")]
    pub ref_type: String,
    /// Catch-all for extra reference fields.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Plan reference from the job message.
/// Maps to C# `TaskOrchestrationPlanReference`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReference {
    #[serde(default)]
    pub scope_identifier: String,
    #[serde(default)]
    pub plan_type: String,
    #[serde(default)]
    pub plan_id: String,
    /// Catch-all for extra plan fields.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Timeline reference from the job message.
/// Maps to C# `TimelineReference`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineReference {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub change_id: i64,
    #[serde(default)]
    pub location: Option<String>,
}

/// Job resources – service endpoints, container registries, repositories.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResources {
    #[serde(default)]
    pub endpoints: Vec<ServiceEndpoint>,
    #[serde(default)]
    pub repositories: Vec<serde_json::Value>,
    #[serde(default)]
    pub containers: Vec<serde_json::Value>,
}

/// A service endpoint for server communication.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEndpoint {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub authorization: Option<EndpointAuthorization>,
    #[serde(default)]
    pub data: std::collections::HashMap<String, String>,
}

/// Authorization data for a service endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointAuthorization {
    #[serde(default)]
    pub scheme: String,
    #[serde(default)]
    pub parameters: std::collections::HashMap<String, String>,
}

/// Workspace information. C# `WorkspaceOptions` only has `clean`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    #[serde(default)]
    pub clean: Option<String>,
}

/// Job container configuration (used after parsing TemplateToken).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobContainerInfo {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub options: Option<String>,
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub credentials: Option<ContainerCredentials>,
}

/// Docker registry credentials.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerCredentials {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

/// The worker service – top level orchestration.
pub struct Worker {
    host_context: Arc<HostContext>,
}

impl Worker {
    /// Create a new `Worker` bound to the given host context.
    pub fn new(host_context: Arc<HostContext>) -> Self {
        Self { host_context }
    }

    /// Main entry point. Connects to the IPC channel, receives the job message,
    /// initializes secrets, runs the job, and returns the resulting `TaskResult`.
    pub async fn run_async(&self, pipe_in: &str, pipe_out: &str) -> Result<TaskResult> {
        let trace = self.host_context.get_trace("Worker");
        trace.info("Connecting to the listener via IPC...");

        // Connect inbound channel (receive messages from listener)
        let mut channel_in = ProcessChannel::new();
        channel_in
            .start_client(pipe_in)
            .await
            .context("Failed to connect inbound IPC channel")?;

        // Connect outbound channel (send messages to listener)
        let mut channel_out = ProcessChannel::new();
        channel_out
            .start_client(pipe_out)
            .await
            .context("Failed to connect outbound IPC channel")?;

        trace.info("Successfully connected to listener IPC channels.");

        // Receive the job message
        let msg = channel_in
            .receive_async()
            .await
            .context("Failed to receive job message from listener")?;

        if msg.message_type != MessageType::NewJobRequest {
            anyhow::bail!(
                "Expected NewJobRequest message, got {}",
                msg.message_type
            );
        }

        trace.info("Received job message from listener.");

        // Log the raw body length for diagnostics
        trace.info(&format!(
            "Job message body size: {} bytes",
            msg.body.len()
        ));

        // Log a preview of the body (first 500 chars) for debugging
        let preview: String = msg.body.chars().take(500).collect();
        trace.info(&format!("Job message preview: {}", preview));

        // Deserialize the job message
        let job_message: AgentJobRequestMessage = match serde_json::from_str(&msg.body) {
            Ok(m) => m,
            Err(e) => {
                trace.error(&format!(
                    "Failed to deserialize AgentJobRequestMessage: {}",
                    e
                ));
                // Log more body context for debugging
                let extended_preview: String = msg.body.chars().take(2000).collect();
                trace.error(&format!("Raw body (first 2000 chars): {}", extended_preview));
                return Err(anyhow::anyhow!(
                    "Failed to deserialize AgentJobRequestMessage: {}",
                    e
                ));
            }
        };

        trace.info(&format!(
            "Job: {} ({})",
            job_message.job_display_name, job_message.job_id
        ));
        trace.info(&format!(
            "Plan ID: {}, Timeline ID: {}",
            job_message.plan_id(),
            job_message.timeline_id()
        ));
        trace.info(&format!(
            "Steps: {}, Variables: {}, Endpoints: {}",
            job_message.steps.len(),
            job_message.variables.len(),
            job_message.resources.endpoints.len()
        ));

        // Initialize secret masker from job variables
        self.initialize_secrets(&job_message);

        // Set up cancellation
        let cancel_token = CancellationToken::new();
        let cancel_child = cancel_token.clone();

        // Spawn a task to listen for cancel messages
        let cancel_handle = {
            let trace_cancel = self.host_context.get_trace("Worker.Cancel");
            tokio::spawn(async move {
                Self::listen_for_cancel(&mut channel_in, cancel_child, trace_cancel).await;
            })
        };

        // Run the job
        let job_runner = JobRunner::new(Arc::clone(&self.host_context));
        let result = job_runner
            .run_async(job_message.clone(), cancel_token.clone())
            .await
            .unwrap_or_else(|e| {
                tracing::error!("JobRunner failed: {:#}", e);
                TaskResult::Failed
            });

        // Report job completion to the server
        // This is critical — without it the server thinks the job is still running
        // and the broker will endlessly flood cancellation messages.
        match RunServer::from_message(&job_message) {
            Ok(run_server) => {
                let report_trace = self.host_context.get_trace("Worker.CompleteJob");
                if let Err(e) = run_server
                    .complete_job(
                        &job_message.plan_id(),
                        &job_message.job_id,
                        result,
                        &report_trace,
                    )
                    .await
                {
                    trace.error(&format!("Failed to report job completion: {:#}", e));
                }
            }
            Err(e) => {
                trace.error(&format!(
                    "Failed to create RunServer from job message: {:#}",
                    e
                ));
            }
        }

        // Cancel the listener task
        cancel_token.cancel();
        let _ = cancel_handle.await;

        // Notify the listener that the job is done
        let result_code = runner_common::util::task_result_util::TaskResultUtil::translate_to_return_code(result);
        let _ = channel_out
            .send_async(MessageType::NewJobRequest, &result_code.to_string())
            .await;

        trace.info(&format!("Worker completed with result: {}", result));

        Ok(result)
    }

    /// Initialize the secret masker from job variables that are marked as secret.
    fn initialize_secrets(&self, message: &AgentJobRequestMessage) {
        let masker = &self.host_context.secret_masker;

        for (_name, var) in &message.variables {
            if var.is_secret && !var.value.is_empty() {
                masker.add_value(&var.value);
            }
        }

        // Also mask authorization parameters from endpoints
        for endpoint in &message.resources.endpoints {
            if let Some(ref auth) = endpoint.authorization {
                for (_key, value) in &auth.parameters {
                    if !value.is_empty() {
                        masker.add_value(value);
                    }
                }
            }
        }

        // Container credentials are in TemplateToken format now.
        // We'll add masking for those when we implement proper container support.
    }

    /// Listen for cancellation / shutdown messages from the listener.
    async fn listen_for_cancel(
        channel: &mut ProcessChannel,
        cancel_token: CancellationToken,
        trace: runner_common::tracing::Tracing,
    ) {
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    trace.info("Cancel listener stopping (token cancelled).");
                    break;
                }
                result = channel.receive_async() => {
                    match result {
                        Ok(msg) => {
                            match msg.message_type {
                                MessageType::CancelRequest => {
                                    trace.info("Received CancelRequest from listener.");
                                    cancel_token.cancel();
                                    break;
                                }
                                MessageType::RunnerShutdown => {
                                    trace.info("Received RunnerShutdown from listener.");
                                    cancel_token.cancel();
                                    break;
                                }
                                MessageType::OperatingSystemShutdown => {
                                    trace.info("Received OperatingSystemShutdown from listener.");
                                    cancel_token.cancel();
                                    break;
                                }
                                other => {
                                    trace.info(&format!("Received unexpected message type: {}", other));
                                }
                            }
                        }
                        Err(e) => {
                            trace.info(&format!("IPC channel read error (likely closed): {}", e));
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_empty_job_message() {
        let json = r#"{"jobId":"abc-123","jobDisplayName":"Test Job"}"#;
        let msg: AgentJobRequestMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.job_id, "abc-123");
        assert_eq!(msg.job_display_name, "Test Job");
        assert!(msg.steps.is_empty());
    }

    #[test]
    fn test_deserialize_with_plan_and_timeline() {
        let json = r#"{
            "jobId": "abc-123",
            "jobDisplayName": "Test Job",
            "plan": {"planId": "plan-1", "scopeIdentifier": "scope-1"},
            "timeline": {"id": "tl-1", "changeId": 5}
        }"#;
        let msg: AgentJobRequestMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.plan_id(), "plan-1");
        assert_eq!(msg.timeline_id(), "tl-1");
    }

    #[test]
    fn test_deserialize_file_table_as_strings() {
        let json = r#"{
            "jobId": "abc-123",
            "fileTable": [".github/workflows/test.yaml", "action.yml"]
        }"#;
        let msg: AgentJobRequestMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.file_table.len(), 2);
        assert_eq!(msg.file_table[0], ".github/workflows/test.yaml");
    }

    #[test]
    fn test_deserialize_variable_value_message() {
        let json = r#"{"value":"secret123","isSecret":true,"isReadOnly":false}"#;
        let var: VariableValueMessage = serde_json::from_str(json).unwrap();
        assert_eq!(var.value, "secret123");
        assert!(var.is_secret);
        assert!(!var.is_read_only);
    }

    #[test]
    fn test_deserialize_job_step() {
        let json = r#"{
            "id": "step1",
            "displayName": "Run tests",
            "condition": "success()",
            "timeoutInMinutes": 30,
            "stepType": "script",
            "inputs": {"script": "echo hello"},
            "continueOnError": false
        }"#;
        let step: JobStep = serde_json::from_str(json).unwrap();
        assert_eq!(step.id, "step1");
        assert_eq!(step.display_name, "Run tests");
        assert_eq!(step.condition, "success()");
        assert_eq!(step.timeout_in_minutes, 30);
    }
}
