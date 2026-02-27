// ResultsClient — client for the GitHub Actions Results Service.
//
// The Results Service uses Twirp-style JSON RPCs. All calls go to the
// ResultsServiceUrl data key from the SystemVssConnection endpoint, with
// Bearer token auth using the same access token.
//
// API calls implemented:
//   1. WorkflowStepsUpdate — report step status (InProgress/Completed)
//   2. GetStepLogsSignedBlobURL — get a SAS URL to upload step logs
//   3. Upload step logs to the SAS URL (plain PUT to Azure blob storage)
//   4. CreateStepLogsMetadata — finalize the log upload with line count

use anyhow::{Context, Result};
use chrono::Utc;
use runner_sdk::TraceWriter;

use crate::worker::AgentJobRequestMessage;

/// Step status values for the Results Service.
/// These match the C# StepStatus enum.
#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub enum StepStatus {
    /// Step is waiting to run.
    Pending = 5,
    /// Step is currently executing.
    InProgress = 3,
    /// Step has finished.
    Completed = 6,
}

/// Step conclusion values for the Results Service.
/// These match the C# StepConclusion enum.
#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub enum StepConclusion {
    /// Not yet determined.
    Unknown = 0,
    /// Step succeeded.
    Success = 2,
    /// Step failed.
    Failure = 3,
    /// Step was cancelled.
    Cancelled = 4,
    /// Step was skipped.
    Skipped = 7,
}

/// Information about a step to update via the Results Service.
pub struct StepUpdate {
    /// The step's GUID (from the job message step.id).
    pub external_id: String,
    /// Step number (1-based order).
    pub number: u32,
    /// Display name of the step.
    pub name: String,
    /// Current status.
    pub status: StepStatus,
    /// When the step started (ISO 8601).
    pub started_at: Option<String>,
    /// When the step completed (ISO 8601).
    pub completed_at: Option<String>,
    /// Conclusion (only meaningful when status == Completed).
    pub conclusion: StepConclusion,
}

/// Client for the GitHub Actions Results Service.
pub struct ResultsClient {
    /// Base URL of the Results Service (from ResultsServiceUrl data key).
    results_url: String,
    /// OAuth access token from the SystemVssConnection endpoint.
    access_token: String,
    /// Plan ID (workflow_run_backend_id).
    plan_id: String,
    /// Job ID (workflow_job_run_backend_id).
    job_id: String,
    /// HTTP client.
    client: reqwest::Client,
}

impl ResultsClient {
    /// Create a ResultsClient from the job message.
    ///
    /// Extracts the Results Service URL from the SystemVssConnection endpoint's
    /// `ResultsServiceUrl` data key, and the access token from its authorization.
    pub fn from_message(message: &AgentJobRequestMessage) -> Result<Self> {
        let endpoint = message
            .resources
            .endpoints
            .iter()
            .find(|e| e.name == "SystemVssConnection")
            .context("No SystemVssConnection endpoint in job message")?;

        let access_token = endpoint
            .authorization
            .as_ref()
            .and_then(|a| a.parameters.get("AccessToken"))
            .context("No AccessToken in SystemVssConnection authorization")?
            .clone();

        let results_url = endpoint
            .data
            .get("ResultsServiceUrl")
            .context("No ResultsServiceUrl in SystemVssConnection data")?
            .trim_end_matches('/')
            .to_string();

        let plan_id = message.plan_id();
        let job_id = message.job_id.clone();

        Ok(Self {
            results_url,
            access_token,
            plan_id,
            job_id,
            client: reqwest::Client::new(),
        })
    }

    /// Update step statuses via the Results Service.
    ///
    /// POST {results_url}/twirp/github.actions.results.api.v1.WorkflowStepUpdateService/WorkflowStepsUpdate
    pub async fn update_workflow_steps(
        &self,
        steps: &[StepUpdate],
        change_order: u64,
        trace: &dyn TraceWriter,
    ) -> Result<()> {
        let url = format!(
            "{}/twirp/github.actions.results.api.v1.WorkflowStepUpdateService/WorkflowStepsUpdate",
            self.results_url
        );

        let steps_json: Vec<serde_json::Value> = steps
            .iter()
            .map(|s| {
                let mut step = serde_json::json!({
                    "external_id": s.external_id,
                    "number": s.number,
                    "name": s.name,
                    "status": s.status as i32,
                    "conclusion": s.conclusion as i32,
                });
                if let Some(ref started) = s.started_at {
                    step["started_at"] = serde_json::json!(started);
                }
                if let Some(ref completed) = s.completed_at {
                    step["completed_at"] = serde_json::json!(completed);
                }
                step
            })
            .collect();

        let body = serde_json::json!({
            "workflow_run_backend_id": self.plan_id,
            "workflow_job_run_backend_id": self.job_id,
            "change_order": change_order,
            "steps": steps_json,
        });

        trace.info(&format!(
            "Updating {} step(s) via Results Service (change_order={})",
            steps.len(),
            change_order
        ));

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send WorkflowStepsUpdate request")?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            trace.error(&format!(
                "WorkflowStepsUpdate failed: HTTP {} - {}",
                status, body_text
            ));
            anyhow::bail!("WorkflowStepsUpdate returned HTTP {}: {}", status, body_text);
        }

        trace.info(&format!(
            "WorkflowStepsUpdate succeeded (HTTP {})",
            status
        ));
        Ok(())
    }

    /// Upload step logs to the Results Service.
    ///
    /// This is a 3-step process:
    /// 1. GetStepLogsSignedBlobURL — get a SAS URL for uploading
    /// 2. PUT the log content to the SAS URL (Azure blob storage)
    /// 3. CreateStepLogsMetadata — finalize with line count
    pub async fn upload_step_log(
        &self,
        step_id: &str,
        log_lines: &[String],
        trace: &dyn TraceWriter,
    ) -> Result<()> {
        if log_lines.is_empty() {
            trace.info("No log lines to upload, skipping.");
            return Ok(());
        }

        // Prefix each line with an ISO 8601 timestamp
        let now = Utc::now();
        let log_content: String = log_lines
            .iter()
            .map(|line| format!("{} {}", now.format("%Y-%m-%dT%H:%M:%S%.3fZ"), line))
            .collect::<Vec<_>>()
            .join("\n");
        let line_count = log_lines.len();

        trace.info(&format!(
            "Uploading step log for step {} ({} lines, {} bytes)",
            step_id,
            line_count,
            log_content.len()
        ));

        // Step 1: Get SAS URL
        let sas_url = self.get_step_logs_signed_blob_url(step_id, trace).await?;

        // Step 2: Upload to blob storage
        self.upload_to_blob(&sas_url, &log_content, trace).await?;

        // Step 3: Finalize metadata
        self.create_step_logs_metadata(step_id, line_count as u64, trace)
            .await?;

        trace.info(&format!(
            "Successfully uploaded {} log lines for step {}",
            line_count, step_id
        ));

        Ok(())
    }

    /// Step 1: Get a signed blob URL for uploading step logs.
    ///
    /// POST {results_url}/twirp/results.services.receiver.Receiver/GetStepLogsSignedBlobURL
    async fn get_step_logs_signed_blob_url(
        &self,
        step_id: &str,
        trace: &dyn TraceWriter,
    ) -> Result<String> {
        let url = format!(
            "{}/twirp/results.services.receiver.Receiver/GetStepLogsSignedBlobURL",
            self.results_url
        );

        let body = serde_json::json!({
            "workflow_run_backend_id": self.plan_id,
            "workflow_job_run_backend_id": self.job_id,
            "step_backend_id": step_id,
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to request step log SAS URL")?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "GetStepLogsSignedBlobURL returned HTTP {}: {}",
                status,
                body_text
            );
        }

        let resp_body: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse GetStepLogsSignedBlobURL response")?;

        let logs_url = resp_body["logs_url"]
            .as_str()
            .context("No logs_url in GetStepLogsSignedBlobURL response")?
            .to_string();

        trace.info("Got step log SAS URL from Results Service");
        Ok(logs_url)
    }

    /// Step 2: Upload log content to Azure blob storage.
    ///
    /// PUT {sas_url}
    /// Content-Type: text/plain
    /// x-ms-blob-type: BlockBlob
    async fn upload_to_blob(
        &self,
        sas_url: &str,
        content: &str,
        trace: &dyn TraceWriter,
    ) -> Result<()> {
        let response = self
            .client
            .put(sas_url)
            .header("Content-Type", "text/plain")
            .header("x-ms-blob-type", "BlockBlob")
            .body(content.to_owned())
            .send()
            .await
            .context("Failed to upload log to blob storage")?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Blob upload returned HTTP {}: {}", status, body_text);
        }

        trace.info(&format!(
            "Log blob uploaded successfully (HTTP {})",
            status
        ));
        Ok(())
    }

    /// Step 3: Finalize the log upload with metadata (line count).
    ///
    /// POST {results_url}/twirp/results.services.receiver.Receiver/CreateStepLogsMetadata
    async fn create_step_logs_metadata(
        &self,
        step_id: &str,
        line_count: u64,
        trace: &dyn TraceWriter,
    ) -> Result<()> {
        let url = format!(
            "{}/twirp/results.services.receiver.Receiver/CreateStepLogsMetadata",
            self.results_url
        );

        let body = serde_json::json!({
            "workflow_run_backend_id": self.plan_id,
            "workflow_job_run_backend_id": self.job_id,
            "step_backend_id": step_id,
            "uploaded_at": Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            "line_count": line_count,
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send CreateStepLogsMetadata request")?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "CreateStepLogsMetadata returned HTTP {}: {}",
                status,
                body_text
            );
        }

        trace.info(&format!(
            "Step log metadata finalized (HTTP {})",
            status
        ));
        Ok(())
    }
}
