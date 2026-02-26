// RunServer mapping `RunServer.cs`.
// Client for the Actions Run Service – reports job completion back to the server.
//
// The Run Service URL comes from the SystemVssConnection endpoint in the job
// message resources.  The access token comes from the same endpoint's OAuth
// authorization parameters.

use anyhow::{Context, Result};
use runner_common::util::task_result_util::TaskResult;
use runner_sdk::TraceWriter;

use crate::worker::AgentJobRequestMessage;

/// Minimal client for the Actions Run Service.
pub struct RunServer {
    /// Base URL of the Run Service (SystemVssConnection endpoint URL).
    base_url: String,
    /// OAuth access token from the SystemVssConnection endpoint.
    access_token: String,
    /// HTTP client
    client: reqwest::Client,
}

impl RunServer {
    /// Create a RunServer from the job message's SystemVssConnection endpoint.
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

        let base_url = endpoint.url.trim_end_matches('/').to_string();

        Ok(Self {
            base_url,
            access_token,
            client: reqwest::Client::new(),
        })
    }

    /// Report job completion to the Actions Run Service.
    ///
    /// POST {base_url}/completejob
    ///
    /// This is the critical call that tells the server the job is done.
    /// Without this, the server considers the job still running and keeps
    /// sending cancellation messages.
    pub async fn complete_job(
        &self,
        plan_id: &str,
        job_id: &str,
        conclusion: TaskResult,
        trace: &dyn TraceWriter,
    ) -> Result<()> {
        let url = format!("{}/completejob", self.base_url);

        // TaskResult enum values map to camelCase string conclusions:
        //   Succeeded → "succeeded", SucceededWithIssues → "succeededWithIssues",
        //   Failed → "failed", Canceled → "canceled", Skipped → "skipped",
        //   Abandoned → "abandoned"
        let conclusion_str = match conclusion {
            TaskResult::Succeeded => "succeeded",
            TaskResult::SucceededWithIssues => "succeededWithIssues",
            TaskResult::Failed => "failed",
            TaskResult::Canceled => "canceled",
            TaskResult::Skipped => "skipped",
            TaskResult::Abandoned => "abandoned",
        };

        let body = serde_json::json!({
            "planId": plan_id,
            "jobId": job_id,
            "conclusion": conclusion_str
        });

        trace.info(&format!(
            "Reporting job completion: planId={}, jobId={}, conclusion={}",
            plan_id, job_id, conclusion_str
        ));

        let mut last_err = None;
        for attempt in 1..=5 {
            match self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.access_token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        trace.info(&format!(
                            "Successfully reported job completion (HTTP {})",
                            status
                        ));
                        return Ok(());
                    }
                    let body_text = response.text().await.unwrap_or_default();
                    trace.warning(&format!(
                        "CompleteJob attempt {}/5 failed: HTTP {} - {}",
                        attempt, status, body_text
                    ));
                    last_err = Some(anyhow::anyhow!(
                        "CompleteJob returned HTTP {}: {}",
                        status,
                        body_text
                    ));
                }
                Err(e) => {
                    trace.warning(&format!(
                        "CompleteJob attempt {}/5 failed: {}",
                        attempt, e
                    ));
                    last_err = Some(e.into());
                }
            }

            if attempt < 5 {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("CompleteJob failed after 5 attempts")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conclusion_values() {
        assert_eq!(TaskResult::Succeeded as i32, 0);
        assert_eq!(TaskResult::SucceededWithIssues as i32, 1);
        assert_eq!(TaskResult::Failed as i32, 2);
        assert_eq!(TaskResult::Canceled as i32, 3);
        assert_eq!(TaskResult::Skipped as i32, 4);
        assert_eq!(TaskResult::Abandoned as i32, 5);
    }
}
