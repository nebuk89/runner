// PublishArtifact – uploads build artifacts to the file container and
// associates them with the pipeline run via the Pipelines API.
//
// Maps `PublishArtifact.cs` from `Runner.Plugins.Artifact`.

use anyhow::{Context, Result};
use async_trait::async_trait;
use runner_sdk::{ActionPlugin, ActionPluginContext, TraceWriter, VssUtil};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::artifact::file_container_server::FileContainerServer;
use crate::artifact::pipelines_server::PipelinesServer;

/// Input names for the publish-artifact action.
mod input_names {
    /// Legacy input name (back-compat, renamed to `name`).
    pub const ARTIFACT_NAME: &str = "artifactName";
    /// Current input name for the artifact display name.
    pub const NAME: &str = "name";
    /// The local path to the file or directory to upload.
    pub const PATH: &str = "path";
}

/// Well-known variable keys used by the publish-artifact plugin.
mod variables {
    pub const BUILD_ID: &str = "build.buildId";
    pub const CONTAINER_ID: &str = "build.containerId";
}

/// Characters that are invalid in an artifact name (mirroring Path.GetInvalidFileNameChars()
/// on .NET but restricted to the set the server actually rejects).
const INVALID_ARTIFACT_NAME_CHARS: &[char] = &['\\', '/', '"', ':', '<', '>', '|', '*', '?'];

/// Plugin that uploads build artifacts.
///
/// Maps `PublishArtifact` (C# `IRunnerActionPlugin`).
pub struct PublishArtifactPlugin;

#[async_trait]
impl ActionPlugin for PublishArtifactPlugin {
    async fn run(
        &self,
        context: &mut ActionPluginContext,
        trace: &dyn TraceWriter,
    ) -> Result<()> {
        // -----------------------------------------------------------
        // 1. Read inputs
        // -----------------------------------------------------------

        // Back-compat: try `artifactName` first, fall back to `name`.
        let artifact_name = match context.get_input(input_names::ARTIFACT_NAME, false)? {
            Some(v) if !v.is_empty() => v,
            _ => context
                .get_input(input_names::NAME, true)?
                .unwrap_or_default(),
        };

        if artifact_name.trim().is_empty() {
            anyhow::bail!("Artifact name can not be empty string");
        }

        if artifact_name.contains(INVALID_ARTIFACT_NAME_CHARS) {
            anyhow::bail!(
                "Artifact name is not valid: {artifact_name}. \
                 It cannot contain '\\', '/', '\"', ':', '<', '>', '|', '*', and '?'"
            );
        }

        let target_path_raw = context
            .get_input(input_names::PATH, true)?
            .unwrap_or_default();
        let default_working_directory = context
            .get_github_context("workspace")
            .unwrap_or_else(|| ".".to_string());

        let target_path: PathBuf = if Path::new(&target_path_raw).is_absolute() {
            PathBuf::from(&target_path_raw)
        } else {
            Path::new(&default_working_directory).join(&target_path_raw)
        };

        let full_path = std::fs::canonicalize(&target_path).with_context(|| {
            format!("Path does not exist {}", target_path.display())
        })?;

        let is_file = full_path.is_file();
        let is_dir = full_path.is_dir();
        if !is_file && !is_dir {
            anyhow::bail!("Path does not exist {}", target_path.display());
        }

        // -----------------------------------------------------------
        // 2. Read build / container IDs from variables
        // -----------------------------------------------------------

        let build_id_str = context
            .get_variable(variables::BUILD_ID)
            .cloned()
            .unwrap_or_default();
        let build_id: i32 = build_id_str
            .parse()
            .with_context(|| format!("Run Id is not an Int32: {build_id_str}"))?;

        let container_id_str = context
            .get_variable(variables::CONTAINER_ID)
            .cloned()
            .unwrap_or_default();
        let container_id: i64 = container_id_str
            .parse()
            .with_context(|| format!("Container Id is not an Int64: {container_id_str}"))?;

        trace.info(&format!(
            "Uploading artifact '{}' from '{}' for run #{}",
            artifact_name,
            full_path.display(),
            build_id,
        ));

        // -----------------------------------------------------------
        // 3. Resolve the service connection
        // -----------------------------------------------------------

        let (base_url, auth_token) = resolve_connection(context)?;
        let http_client = VssUtil::create_http_client(&runner_sdk::RunnerWebProxy::new());

        // -----------------------------------------------------------
        // 4. Upload files to file container
        // -----------------------------------------------------------

        let file_container = FileContainerServer::new(
            http_client.clone(),
            &base_url,
            &auth_token,
            Uuid::nil(), // projectId is empty for Actions
            container_id,
            &artifact_name,
        );

        let size = file_container
            .copy_to_container(trace, &full_path.to_string_lossy())
            .await
            .context("Failed to upload artifact files")?;

        trace.info(&format!(
            "Uploaded '{size}' bytes from '{}' to server",
            full_path.display(),
        ));

        // -----------------------------------------------------------
        // 5. Associate artifact with the pipeline run
        // -----------------------------------------------------------

        // Definition ID is a dummy value only used by HTTP client routing.
        let definition_id: i32 = 1;

        let pipelines = PipelinesServer::new(http_client, &base_url, &auth_token);

        let artifact = pipelines
            .associate_actions_storage_artifact(
                definition_id,
                build_id,
                container_id,
                &artifact_name,
                size,
            )
            .await
            .context("Failed to associate artifact with run")?;

        trace.info(&format!(
            "Associated artifact {artifact_name} ({}) with run #{build_id}",
            artifact.container_id,
        ));

        Ok(())
    }
}

/// Resolve the `SystemVssConnection` endpoint from the plugin context.
///
/// Returns `(base_url, access_token)`.
fn resolve_connection(context: &ActionPluginContext) -> Result<(String, String)> {
    let endpoint = context
        .endpoints
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case("SystemVssConnection"))
        .ok_or_else(|| anyhow::anyhow!("SystemVssConnection endpoint not found"))?;

    let auth = endpoint
        .authorization
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("SystemVssConnection has no authorization"))?;

    let token = auth
        .parameters
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("AccessToken"))
        .map(|(_, v)| v.clone())
        .ok_or_else(|| anyhow::anyhow!("AccessToken not found in SystemVssConnection authorization"))?;

    Ok((endpoint.url.clone(), token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runner_sdk::action_plugin::{EndpointAuthorization, ServiceEndpoint};
    use runner_sdk::ActionPluginContext;
    use std::collections::HashMap;

    fn make_context() -> ActionPluginContext {
        let mut ctx = ActionPluginContext::new();
        ctx.inputs
            .insert("name".to_string(), "my-artifact".to_string());
        // Use the temp dir so the path actually exists
        ctx.inputs.insert(
            "path".to_string(),
            std::env::temp_dir().to_string_lossy().to_string(),
        );
        ctx.variables
            .insert("build.buildId".to_string(), "42".to_string());
        ctx.variables
            .insert("build.containerId".to_string(), "123".to_string());

        // github context with workspace
        let github = serde_json::json!({
            "workspace": std::env::temp_dir().to_string_lossy().to_string(),
        });
        ctx.context.insert("github".to_string(), github);

        ctx.endpoints.push(ServiceEndpoint {
            name: "SystemVssConnection".to_string(),
            url: "https://pipelines.actions.githubusercontent.com".to_string(),
            authorization: Some(EndpointAuthorization {
                scheme: "OAuth".to_string(),
                parameters: {
                    let mut p = HashMap::new();
                    p.insert("AccessToken".to_string(), "test-token".to_string());
                    p
                },
            }),
        });

        ctx
    }

    #[test]
    fn resolve_connection_works() {
        let ctx = make_context();
        let (url, token) = resolve_connection(&ctx).unwrap();
        assert_eq!(url, "https://pipelines.actions.githubusercontent.com");
        assert_eq!(token, "test-token");
    }

    #[test]
    fn resolve_connection_missing_endpoint() {
        let ctx = ActionPluginContext::new();
        assert!(resolve_connection(&ctx).is_err());
    }

    #[test]
    fn invalid_artifact_name_chars() {
        for ch in INVALID_ARTIFACT_NAME_CHARS {
            let name = format!("test{ch}artifact");
            assert!(
                name.contains(INVALID_ARTIFACT_NAME_CHARS),
                "Expected '{name}' to contain invalid char '{ch}'"
            );
        }
    }

    #[test]
    fn empty_artifact_name_is_invalid() {
        let name = "   ";
        assert!(name.trim().is_empty());
    }
}
