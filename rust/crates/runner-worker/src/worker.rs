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

/// Deserialized job request message from the listener.
/// Maps `Pipelines.AgentJobRequestMessage` from the C# runner.
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

    /// Plan ID for the orchestration plan.
    #[serde(default)]
    pub plan_id: String,

    /// Timeline ID for log uploads.
    #[serde(default)]
    pub timeline_id: String,

    /// Job-level environment variables.
    #[serde(default)]
    pub environment_variables: std::collections::HashMap<String, String>,

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
    pub workspace: Option<WorkspaceInfo>,

    /// File table entries (for file commands).
    #[serde(default)]
    pub file_table: Vec<FileTableEntry>,

    /// Context data (github, runner, needs, strategy, matrix, inputs).
    #[serde(default)]
    pub context_data: std::collections::HashMap<String, serde_json::Value>,

    /// Job container definition.
    #[serde(default)]
    pub job_container: Option<JobContainerInfo>,

    /// Service containers.
    #[serde(default)]
    pub job_service_containers: Vec<JobContainerInfo>,

    /// Actor requesting the workflow.
    #[serde(default)]
    pub actor: String,

    /// Message type discriminator.
    #[serde(default)]
    pub message_type: String,
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
    #[serde(default)]
    pub timeout_in_minutes: u32,

    /// Type of step: "script", "action", etc.
    #[serde(default)]
    pub step_type: String,

    /// Reference for action steps (e.g. "actions/checkout@v4").
    #[serde(default)]
    pub reference: Option<ActionReference>,

    /// Inline inputs / with values.
    #[serde(default)]
    pub inputs: std::collections::HashMap<String, String>,

    /// Step-level environment variables.
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,

    /// Continue on error flag.
    #[serde(default)]
    pub continue_on_error: bool,

    /// The script body for run steps.
    #[serde(default)]
    pub script: Option<String>,

    /// Shell override (bash, pwsh, python, etc.).
    #[serde(default)]
    pub shell: Option<String>,

    /// Working directory override.
    #[serde(default)]
    pub working_directory: Option<String>,
}

/// Action reference in a step.
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
}

/// Job resources – service endpoints.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResources {
    #[serde(default)]
    pub endpoints: Vec<ServiceEndpoint>,
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

/// Workspace information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    #[serde(default)]
    pub clean: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
}

/// File table entry for file commands (GITHUB_ENV, GITHUB_PATH, etc.).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTableEntry {
    #[serde(default)]
    pub file_table_id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub path: String,
}

/// Job container configuration.
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

        // Deserialize the job message
        let job_message: AgentJobRequestMessage = serde_json::from_str(&msg.body)
            .context("Failed to deserialize AgentJobRequestMessage")?;

        trace.info(&format!(
            "Job: {} ({})",
            job_message.job_display_name, job_message.job_id
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
            .run_async(job_message, cancel_token.clone())
            .await
            .unwrap_or_else(|e| {
                tracing::error!("JobRunner failed: {:#}", e);
                TaskResult::Failed
            });

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

        // Mask container credentials
        if let Some(ref container) = message.job_container {
            if let Some(ref creds) = container.credentials {
                if !creds.password.is_empty() {
                    masker.add_value(&creds.password);
                }
            }
        }
        for svc in &message.job_service_containers {
            if let Some(ref creds) = svc.credentials {
                if !creds.password.is_empty() {
                    masker.add_value(&creds.password);
                }
            }
        }
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
