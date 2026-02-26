// PipelinesServer – a client wrapper interacting with the Pipelines Artifact API.
//
// Maps `PipelinesServer.cs` from `Runner.Plugins.Artifact`.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// An artifact stored in Actions Storage (file container backed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionsStorageArtifact {
    /// The display name of the artifact.
    pub name: String,

    /// The file container ID backing this artifact.
    #[serde(default)]
    pub container_id: i64,

    /// Size in bytes (populated after finalization).
    #[serde(default)]
    pub size: i64,
}

/// Parameters for creating an Actions Storage artifact.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateActionsStorageArtifactParameters {
    name: String,
    container_id: i64,
    size: i64,
    /// Discriminator – tells the server this is an Actions Storage artifact.
    #[serde(rename = "type")]
    artifact_type: String,
}

/// A wrapper around the Pipelines HTTP API for artifact operations.
///
/// Maps `PipelinesServer` from the C# codebase.
#[derive(Debug)]
pub struct PipelinesServer {
    client: Client,
    base_url: String,
    auth_token: String,
}

impl PipelinesServer {
    /// Create a new `PipelinesServer`.
    ///
    /// * `client`     – a pre-configured `reqwest::Client`
    /// * `base_url`   – the base URL of the Pipelines service (e.g. `https://pipelines.actions.githubusercontent.com`)
    /// * `auth_token` – the OAuth access token for authentication
    pub fn new(client: Client, base_url: &str, auth_token: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_token: auth_token.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Internal: build the REST URL for pipelines artifacts
    // -----------------------------------------------------------------------

    fn artifacts_url(&self, _pipeline_id: i32, run_id: i32) -> String {
        format!(
            "{base}/_apis/pipelines/workflows/{run_id}/artifacts?api-version=6.0-preview",
            base = self.base_url,
            run_id = run_id,
        )
    }

    fn artifact_by_name_url(&self, _pipeline_id: i32, run_id: i32, name: &str) -> String {
        format!(
            "{base}/_apis/pipelines/workflows/{run_id}/artifacts?artifactName={name}&api-version=6.0-preview",
            base = self.base_url,
            run_id = run_id,
            name = percent_encoding::utf8_percent_encode(
                name,
                percent_encoding::NON_ALPHANUMERIC,
            ),
        )
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Associate an Actions Storage artifact with a pipeline run.
    ///
    /// This calls the `POST artifacts` endpoint to create / associate the
    /// artifact record after the files have been uploaded to the file container.
    pub async fn associate_actions_storage_artifact(
        &self,
        pipeline_id: i32,
        run_id: i32,
        container_id: i64,
        name: &str,
        size: i64,
    ) -> Result<ActionsStorageArtifact> {
        let url = self.artifacts_url(pipeline_id, run_id);

        let body = CreateActionsStorageArtifactParameters {
            name: name.to_string(),
            container_id,
            size,
            artifact_type: "actions_storage".to_string(),
        };

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.auth_token)
            .json(&body)
            .send()
            .await
            .context("Failed to send associate artifact request")?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to associate artifact '{name}' (HTTP {status}): {text}"
            );
        }

        let artifact: ActionsStorageArtifact = response
            .json()
            .await
            .context("Failed to deserialize artifact response")?;

        Ok(artifact)
    }

    /// Get a named Actions Storage artifact for a pipeline run.
    ///
    /// Returns `None` if the artifact does not exist (HTTP 404 or empty result).
    pub async fn get_actions_storage_artifact(
        &self,
        pipeline_id: i32,
        run_id: i32,
        name: &str,
    ) -> Result<Option<ActionsStorageArtifact>> {
        let url = self.artifact_by_name_url(pipeline_id, run_id, name);

        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .context("Failed to send get artifact request")?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to get artifact '{name}' (HTTP {status}): {text}"
            );
        }

        let artifact: ActionsStorageArtifact = response
            .json()
            .await
            .context("Failed to deserialize artifact response")?;

        Ok(Some(artifact))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_url_format() {
        let server = PipelinesServer::new(
            Client::new(),
            "https://pipelines.actions.githubusercontent.com",
            "tok",
        );
        let url = server.artifacts_url(1, 42);
        assert!(url.contains("/workflows/42/artifacts"));
        assert!(url.contains("api-version=6.0-preview"));
    }

    #[test]
    fn artifact_by_name_url_encodes_name() {
        let server = PipelinesServer::new(Client::new(), "https://example.com", "tok");
        let url = server.artifact_by_name_url(1, 10, "my artifact");
        // space should be percent-encoded
        assert!(url.contains("artifactName=my%20artifact"));
    }

    #[test]
    fn serialization_of_create_params() {
        let params = CreateActionsStorageArtifactParameters {
            name: "test".to_string(),
            container_id: 123,
            size: 456,
            artifact_type: "actions_storage".to_string(),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"containerId\":123"));
        assert!(json.contains("\"type\":\"actions_storage\""));
    }
}
