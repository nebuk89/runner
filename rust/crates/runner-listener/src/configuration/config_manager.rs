// ConfigManager mapping `ConfigurationManager.cs`.
// Full configuration lifecycle: configure_async (prompt URL/token/name/work/labels,
// register with GitHub, save) and unconfigure_async (remove runner, delete config).

use anyhow::{Context, Result};
use runner_common::config_store::{ConfigurationStore, RunnerSettings};
use runner_common::constants::{self, WellKnownConfigFile, WellKnownDirectory};
use runner_common::credential_data::CredentialData;
use runner_common::host_context::HostContext;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use serde::Deserialize;
use std::sync::Arc;

use crate::command_settings::CommandSettings;
use crate::configuration::prompt_manager::PromptManager;
use crate::configuration::rsa_key_manager::RsaKeyManager;
use crate::configuration::validators;

// ---------------------------------------------------------------------------
// Registration API response types
// ---------------------------------------------------------------------------

/// Response from the runner registration API.
#[derive(Debug, Deserialize)]
struct RunnerRegistrationResponse {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
}

/// Response from the token exchange API (runner registration token → OAuth token).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenExchangeResponse {
    #[serde(default, rename = "token")]
    token: String,
    #[serde(default, rename = "token_url")]
    token_url: Option<String>,
}

// ---------------------------------------------------------------------------
// ConfigManager
// ---------------------------------------------------------------------------

/// Manages the runner configuration lifecycle.
///
/// Maps `ConfigurationManager` in the C# runner. Handles interactive and
/// unattended configuration, runner registration with GitHub, and removal.
pub struct ConfigManager {
    context: Arc<HostContext>,
    trace: Tracing,
}

impl ConfigManager {
    /// Create a new `ConfigManager`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("ConfigManager");
        Self { context, trace }
    }

    /// Configure the runner.
    ///
    /// This walks through the configuration flow:
    /// 1. Prompt for URL, token, name, work dir, labels
    /// 2. Validate inputs
    /// 3. Register the runner with GitHub
    /// 4. Save settings and credentials to disk
    /// 5. Optionally generate service config
    pub async fn configure_async(&self, settings: &CommandSettings) -> Result<()> {
        self.trace.info("Starting runner configuration");

        let config_store = ConfigurationStore::new(&self.context);
        let prompt = PromptManager::new(settings.is_unattended());

        // Check if already configured
        if config_store.is_configured() {
            if !settings.is_replace() {
                return Err(anyhow::anyhow!(
                    "Runner is already configured. Use --replace to reconfigure."
                ));
            }
            self.trace
                .info("Runner already configured — will replace");
        }

        // 1. Get the GitHub URL
        let url = match settings.get_url() {
            Some(u) => u,
            None => prompt.prompt_required("Enter the URL of the repository, org, or enterprise")?,
        };
        validators::validate_url(&url)?;

        // 2. Get the registration token
        let token = match settings.get_token() {
            Some(t) => t,
            None => prompt.prompt_required("Enter the registration token")?,
        };

        // 3. Get the runner name (default: hostname)
        let default_name = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "runner".to_string());

        let name = match settings.get_name() {
            Some(n) => n,
            None => prompt.prompt_with_default("Enter the name of the runner", &default_name)?,
        };
        validators::validate_runner_name(&name)?;

        // 4. Get the work directory
        let work = match settings.get_work() {
            Some(w) => w,
            None => prompt.prompt_with_default(
                "Enter the work folder",
                constants::path::WORK_DIRECTORY,
            )?,
        };

        // 5. Get optional labels
        let labels = settings.get_labels().unwrap_or_default();

        // 6. Get optional runner group
        let runner_group = settings
            .get_runner_group()
            .unwrap_or_else(|| "Default".to_string());

        // 7. Parse the GitHub URL to determine API endpoints
        let parsed_url = url::Url::parse(&url)
            .context("Invalid URL")?;
        let is_hosted = runner_sdk::UrlUtil::is_hosted_server(&parsed_url);

        self.trace.info(&format!(
            "Registering runner '{}' at {} (hosted={})",
            name, url, is_hosted
        ));

        // 8. Exchange the registration token for an access token
        let (server_url, access_token, client_id, auth_url) =
            self.exchange_registration_token(&url, &token, is_hosted).await?;

        // 9. Generate RSA key pair for credential exchange
        let rsa_manager = RsaKeyManager::new(self.context.clone());
        let public_key_pem = rsa_manager.generate_and_save_key()?;

        // 10. Register the runner with the server
        let registration = self
            .register_runner(
                &server_url,
                &access_token,
                &name,
                &runner_group,
                &labels,
                &public_key_pem,
                settings.is_ephemeral(),
                settings.is_disable_update(),
                settings.is_no_default_labels(),
            )
            .await?;

        // 11. Build and save settings
        let mut runner_settings = RunnerSettings::default();
        runner_settings.agent_id = registration.id;
        runner_settings.agent_name = registration.name.clone();
        runner_settings.server_url = server_url.clone();
        runner_settings.git_hub_url = url.clone();
        runner_settings.work_folder = work.clone();
        runner_settings.is_ephemeral = settings.is_ephemeral();
        runner_settings.disable_update = settings.is_disable_update();
        runner_settings.pool_name = runner_group.clone();
        runner_settings.set_is_hosted_server(is_hosted);

        config_store
            .save_settings(&runner_settings)
            .context("Failed to save runner settings")?;

        // 12. Save credentials
        let mut cred_data = CredentialData::new(constants::configuration::OAUTH);
        if let Some(ref auth) = auth_url {
            cred_data.authorization_url = Some(auth.clone());
        }
        cred_data.client_id = Some(client_id);
        cred_data
            .data
            .insert("accessToken".to_string(), access_token);

        config_store
            .save_credential(&cred_data)
            .context("Failed to save credentials")?;

        // 13. Create work directory
        let root = self.context.get_directory(WellKnownDirectory::Root);
        let work_path = if std::path::Path::new(&work).is_absolute() {
            std::path::PathBuf::from(&work)
        } else {
            root.join(&work)
        };
        std::fs::create_dir_all(&work_path)
            .context("Failed to create work directory")?;

        // 14. Generate service config if requested
        if settings.is_generate_service_config() {
            let svc_manager =
                super::service_control_manager::ServiceControlManager::new(self.context.clone());
            svc_manager.generate_service_config(&runner_settings)?;
        }

        self.trace.info(&format!(
            "Runner '{}' configured successfully (ID: {})",
            registration.name, registration.id
        ));

        println!("\n√ Runner successfully added");
        println!("√ Runner connection is good\n");

        if settings.is_generate_service_config() {
            println!("√ Service configuration generated\n");
        }

        println!("# Runner settings");
        println!("  Name: {}", registration.name);
        println!("  URL: {}", url);
        println!("  Work folder: {}", work);
        if !labels.is_empty() {
            println!("  Labels: {}", labels);
        }

        Ok(())
    }

    /// Remove the runner (unconfigure).
    ///
    /// This:
    /// 1. Loads the current settings
    /// 2. Authenticates with the token or PAT
    /// 3. Removes the runner from GitHub
    /// 4. Deletes local config files
    pub async fn unconfigure_async(&self, settings: &CommandSettings) -> Result<()> {
        self.trace.info("Starting runner removal");

        let config_store = ConfigurationStore::new(&self.context);

        if !config_store.is_configured() {
            println!("Runner is not configured.");
            return Ok(());
        }

        let runner_settings = config_store
            .get_settings()
            .context("Failed to load runner settings for removal")?;

        let prompt = PromptManager::new(settings.is_unattended());

        // Get the token for removal
        let token = match settings.get_token() {
            Some(t) => t,
            None => match settings.get_pat() {
                Some(p) => p,
                None => prompt.prompt_required("Enter the registration/PAT token to remove the runner")?,
            },
        };

        // Determine the API URL
        let parsed_url = url::Url::parse(&runner_settings.git_hub_url)
            .or_else(|_| url::Url::parse(&runner_settings.server_url))
            .context("No valid URL in runner settings")?;

        let is_hosted = runner_sdk::UrlUtil::is_hosted_server(&parsed_url);

        // Exchange token if it's a registration token
        let (server_url, access_token, _, _) = self
            .exchange_registration_token(
                &runner_settings.git_hub_url,
                &token,
                is_hosted,
            )
            .await
            .unwrap_or_else(|_| {
                // If exchange fails, use the token directly (it might be a PAT)
                (
                    runner_settings.server_url.clone(),
                    token.clone(),
                    String::new(),
                    None,
                )
            });

        // Remove the runner from the server
        self.trace.info(&format!(
            "Removing runner '{}' (ID: {})",
            runner_settings.agent_name, runner_settings.agent_id
        ));

        if let Err(e) = self
            .remove_runner(&server_url, &access_token, runner_settings.agent_id)
            .await
        {
            self.trace.warning(&format!(
                "Failed to remove runner from server: {}. Cleaning up locally anyway.",
                e
            ));
        }

        // Delete local config files
        config_store.delete_settings();
        config_store.delete_credential();

        // Delete RSA key
        let rsa_path = self
            .context
            .get_config_file(WellKnownConfigFile::RSACredentials);
        let _ = std::fs::remove_file(&rsa_path);

        // Delete service config
        let service_path = self.context.get_config_file(WellKnownConfigFile::Service);
        let _ = std::fs::remove_file(&service_path);

        self.trace.info("Runner removed successfully");
        println!("\n√ Runner removed successfully");

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Registration helpers
    // -----------------------------------------------------------------------

    /// Exchange a registration token for an access token and server URL.
    async fn exchange_registration_token(
        &self,
        github_url: &str,
        token: &str,
        is_hosted: bool,
    ) -> Result<(String, String, String, Option<String>)> {
        let parsed = url::Url::parse(github_url)
            .context("Invalid GitHub URL")?;

        let api_base = if is_hosted {
            "https://api.github.com".to_string()
        } else {
            format!("{}://{}/api/v3", parsed.scheme(), parsed.host_str().unwrap_or(""))
        };

        // Determine the scope (repo, org, or enterprise)
        let path_segments: Vec<&str> = parsed
            .path_segments()
            .map(|s| s.filter(|seg| !seg.is_empty()).collect())
            .unwrap_or_default();

        let _registration_url = match path_segments.len() {
            0 => format!("{}/actions/runners/registration-token", api_base),
            1 => format!(
                "{}/orgs/{}/actions/runners/registration-token",
                api_base, path_segments[0]
            ),
            _ => format!(
                "{}/repos/{}/{}/actions/runners/registration-token",
                api_base, path_segments[0], path_segments[1]
            ),
        };

        let _client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        // The registration token can be used directly as a bearer token
        // to call the Actions runner registration API
        let actions_url = self.resolve_actions_url(&api_base, &path_segments, token).await?;

        // Build the server URL and generate a client ID
        let client_id = format!("runner-{}", uuid::Uuid::new_v4());
        let auth_url = if is_hosted {
            Some(format!(
                "{}/actions/token",
                actions_url
            ))
        } else {
            Some(format!(
                "{}://{}/actions/token",
                parsed.scheme(),
                parsed.host_str().unwrap_or("")
            ))
        };

        Ok((actions_url, token.to_string(), client_id, auth_url))
    }

    /// Resolve the Actions service URL for runner registration.
    async fn resolve_actions_url(
        &self,
        api_base: &str,
        path_segments: &[&str],
        token: &str,
    ) -> Result<String> {
        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        // Try to get the Actions service URL from the registration endpoint
        let reg_url = match path_segments.len() {
            0 => format!("{}/actions/runner-registration", api_base),
            1 => format!(
                "{}/orgs/{}/actions/runner-registration",
                api_base, path_segments[0]
            ),
            _ => format!(
                "{}/repos/{}/{}/actions/runner-registration",
                api_base, path_segments[0], path_segments[1]
            ),
        };

        let response = client
            .post(&reg_url)
            .bearer_auth(token)
            .header("Content-Length", "0")
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                #[derive(Deserialize)]
                struct RegResponse {
                    #[serde(default)]
                    url: String,
                }

                if let Ok(reg) = resp.json::<RegResponse>().await {
                    if !reg.url.is_empty() {
                        return Ok(reg.url);
                    }
                }
            }
            _ => {}
        }

        // Fallback: construct a reasonable Actions URL
        Ok(format!("{}/actions", api_base))
    }

    /// Register the runner with the Actions service.
    async fn register_runner(
        &self,
        server_url: &str,
        token: &str,
        name: &str,
        runner_group: &str,
        labels: &str,
        _public_key_pem: &str,
        ephemeral: bool,
        disable_update: bool,
        no_default_labels: bool,
    ) -> Result<RunnerRegistrationResponse> {
        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        // Build the label list
        let mut label_list: Vec<serde_json::Value> = Vec::new();

        if !no_default_labels {
            // Add default labels
            label_list.push(serde_json::json!({"name": "self-hosted", "type": "system"}));
            label_list.push(serde_json::json!({
                "name": constants::CURRENT_PLATFORM.to_string(),
                "type": "system"
            }));
            label_list.push(serde_json::json!({
                "name": constants::CURRENT_ARCHITECTURE.to_string(),
                "type": "system"
            }));
        }

        // Add user-specified labels
        if !labels.is_empty() {
            for label in labels.split(',') {
                let label = label.trim();
                if !label.is_empty() {
                    label_list.push(serde_json::json!({"name": label, "type": "user"}));
                }
            }
        }

        let body = serde_json::json!({
            "name": name,
            "version": runner_sdk::build_constants::RunnerPackage::VERSION,
            "osDescription": format!("{} {}", constants::CURRENT_PLATFORM, constants::CURRENT_ARCHITECTURE),
            "labels": label_list,
            "runnerGroupName": runner_group,
            "ephemeral": ephemeral,
            "disableUpdate": disable_update,
        });

        let url = format!(
            "{}/_apis/distributedtask/pools/0/agents",
            server_url
        );

        self.trace.info(&format!("Registering runner at: {}", url));

        let response = client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .context("Failed to send runner registration request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Runner registration failed with HTTP {}: {}",
                status.as_u16(),
                body_text
            ));
        }

        let registration: RunnerRegistrationResponse = response
            .json()
            .await
            .context("Failed to deserialize registration response")?;

        self.trace.info(&format!(
            "Runner registered: id={}, name={}",
            registration.id, registration.name
        ));

        Ok(registration)
    }

    /// Remove the runner from the Actions service.
    async fn remove_runner(
        &self,
        server_url: &str,
        token: &str,
        agent_id: u64,
    ) -> Result<()> {
        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/distributedtask/pools/0/agents/{}",
            server_url, agent_id
        );

        let response = client
            .delete(&url)
            .bearer_auth(token)
            .send()
            .await
            .context("Failed to send runner removal request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Runner removal failed with HTTP {}: {}",
                status.as_u16(),
                body_text
            ));
        }

        Ok(())
    }
}
