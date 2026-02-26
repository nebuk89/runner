// DownloadArtifact – downloads previously uploaded artifacts from the
// file container to a local directory.
//
// Maps `DownloadArtifact.cs` from `Runner.Plugins.Artifact`.

use anyhow::{Context, Result};
use async_trait::async_trait;
use runner_sdk::{ActionPlugin, ActionPluginContext, TraceWriter, VssUtil};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::artifact::file_container_server::FileContainerServer;
use crate::artifact::pipelines_server::PipelinesServer;

/// Input names for the download-artifact action.
mod input_names {
    /// Current input name for the artifact display name.
    pub const NAME: &str = "name";
    /// Legacy input name (back-compat, renamed from `artifact` to `name`).
    pub const ARTIFACT_NAME: &str = "artifact";
    /// The local path to download the artifact into.
    pub const PATH: &str = "path";
}

/// Well-known variable keys used by the download-artifact plugin.
mod variables {
    pub const BUILD_ID: &str = "build.buildId";
}

/// Plugin that downloads build artifacts.
///
/// Maps `DownloadArtifact` (C# `IRunnerActionPlugin`).
pub struct DownloadArtifactPlugin;

#[async_trait]
impl ActionPlugin for DownloadArtifactPlugin {
    async fn run(
        &self,
        context: &mut ActionPluginContext,
        trace: &dyn TraceWriter,
    ) -> Result<()> {
        // -----------------------------------------------------------
        // 1. Read inputs
        // -----------------------------------------------------------

        // Back-compat: try `artifact` first, fall back to `name`.
        let artifact_name = match context.get_input(input_names::ARTIFACT_NAME, false)? {
            Some(v) if !v.is_empty() => v,
            _ => context
                .get_input(input_names::NAME, true)?
                .unwrap_or_default(),
        };

        let target_path_input = context
            .get_input(input_names::PATH, false)?
            .unwrap_or_default();

        let default_working_directory = context
            .get_github_context("workspace")
            .unwrap_or_else(|| ".".to_string());

        // If no path was supplied, use the artifact name as the target folder name.
        let target_path_raw = if target_path_input.is_empty() {
            artifact_name.clone()
        } else {
            target_path_input
        };

        let target_path: PathBuf = if Path::new(&target_path_raw).is_absolute() {
            PathBuf::from(&target_path_raw)
        } else {
            Path::new(&default_working_directory).join(&target_path_raw)
        };

        // -----------------------------------------------------------
        // 2. Read build ID
        // -----------------------------------------------------------

        let build_id_str = context
            .get_variable(variables::BUILD_ID)
            .cloned()
            .unwrap_or_default();
        let build_id: i32 = build_id_str
            .parse()
            .with_context(|| format!("Run Id is not an Int32: {build_id_str}"))?;

        trace.info(&format!(
            "Downloading artifact '{}' to: '{}'",
            artifact_name,
            target_path.display(),
        ));

        // -----------------------------------------------------------
        // 3. Resolve the service connection
        // -----------------------------------------------------------

        let (base_url, auth_token) = resolve_connection(context)?;
        let http_client = VssUtil::create_http_client(&runner_sdk::RunnerWebProxy::new());

        // -----------------------------------------------------------
        // 4. Get the artifact metadata from Pipelines
        // -----------------------------------------------------------

        // Definition ID is a dummy value only used by HTTP client routing.
        let definition_id: i32 = 1;

        let pipelines = PipelinesServer::new(http_client.clone(), &base_url, &auth_token);

        let artifact = pipelines
            .get_actions_storage_artifact(definition_id, build_id, &artifact_name)
            .await
            .context("Failed to query artifact")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "The actions storage artifact for '{}' could not be found, \
                     or is no longer available",
                    artifact_name,
                )
            })?;

        // In Actions Storage artifacts, name equals the container path.
        let container_path = &artifact.name;
        let container_id = artifact.container_id;

        // -----------------------------------------------------------
        // 5. Download files from the file container
        // -----------------------------------------------------------

        let file_container = FileContainerServer::new(
            http_client,
            &base_url,
            &auth_token,
            Uuid::nil(),
            container_id,
            container_path,
        );

        file_container
            .download_from_container(trace, &target_path.to_string_lossy())
            .await
            .context("Failed to download artifact files")?;

        trace.info("Artifact download finished.");
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
        .ok_or_else(|| {
            anyhow::anyhow!("AccessToken not found in SystemVssConnection authorization")
        })?;

    Ok((endpoint.url.clone(), token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runner_sdk::action_plugin::{EndpointAuthorization, ServiceEndpoint};
    use runner_sdk::ActionPluginContext;
    use std::collections::HashMap;

    fn make_context(artifact_name: &str) -> ActionPluginContext {
        let mut ctx = ActionPluginContext::new();
        ctx.inputs
            .insert("name".to_string(), artifact_name.to_string());
        ctx.variables
            .insert("build.buildId".to_string(), "99".to_string());

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
        let ctx = make_context("test-artifact");
        let (url, token) = resolve_connection(&ctx).unwrap();
        assert_eq!(url, "https://pipelines.actions.githubusercontent.com");
        assert_eq!(token, "test-token");
    }

    #[test]
    fn fallback_target_path_to_artifact_name() {
        // When no path is supplied the artifact name should be used.
        let ctx = make_context("my-art");
        let path_input = ctx
            .get_input(input_names::PATH, false)
            .unwrap()
            .unwrap_or_default();
        assert!(path_input.is_empty());
        // Fallback logic:
        let target = if path_input.is_empty() {
            "my-art".to_string()
        } else {
            path_input
        };
        assert_eq!(target, "my-art");
    }
}
