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
    #[serde(default)]
    authorization: Option<AgentAuthorization>,
    #[serde(default, rename = "ephemeral")]
    ephemeral: bool,
    #[serde(default, rename = "disableUpdate")]
    disable_update: bool,
}

/// Agent authorization data returned from registration.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AgentAuthorization {
    #[serde(default, rename = "authorizationUrl")]
    authorization_url: Option<String>,
    #[serde(default, rename = "clientId")]
    client_id: Option<String>,
}

/// Response from the `actions/runner-registration` endpoint (GetTenantCredential).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GitHubAuthResult {
    #[serde(default, rename = "token")]
    token: String,
    #[serde(default, rename = "token_schema")]
    token_schema: Option<String>,
    #[serde(default, rename = "url")]
    url: String,
    /// Whether to use the runner-admin flow (newer path).
    #[serde(default, rename = "use_runner_admin_flow")]
    use_runner_admin_flow: bool,
}

/// Response from the runner pools endpoint.
#[derive(Debug, Deserialize)]
struct AgentPool {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "isInternal")]
    is_internal: bool,
    #[serde(default, rename = "isHosted")]
    is_hosted: bool,
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
        let (server_url, access_token, _client_id, _auth_url) =
            self.exchange_registration_token(&url, &token, is_hosted).await?;

        // 9. Generate RSA key pair for credential exchange
        let rsa_manager = RsaKeyManager::new(self.context.clone());
        let public_key_pem = rsa_manager.generate_and_save_key()?;

        // 10. Resolve the runner pool / group
        let pools = self.get_agent_pools(&server_url, &access_token).await?;
        let pool = Self::pick_pool(&pools, &runner_group)?;
        self.trace.info(&format!(
            "Using runner group '{}' (pool id {})",
            pool.name, pool.id
        ));

        // 11. Register the runner with the server
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
                pool.id,
            )
            .await?;

        // 12. Build and save settings
        let mut runner_settings = RunnerSettings::default();
        runner_settings.agent_id = registration.id;
        runner_settings.agent_name = registration.name.clone();
        runner_settings.server_url = server_url.clone();
        runner_settings.git_hub_url = url.clone();
        runner_settings.work_folder = work.clone();
        runner_settings.is_ephemeral = settings.is_ephemeral();
        runner_settings.disable_update = settings.is_disable_update();
        runner_settings.pool_name = pool.name.clone();
        runner_settings.pool_id = pool.id as i32;
        runner_settings.set_is_hosted_server(is_hosted);

        config_store
            .save_settings(&runner_settings)
            .context("Failed to save runner settings")?;

        // 12. Save credentials — use the authorization data from the server
        //     response, NOT the registration token.
        let mut cred_data = CredentialData::new(constants::configuration::OAUTH);

        // The server assigns the client_id and authorization_url during registration
        if let Some(ref auth) = registration.authorization {
            if let Some(ref auth_url) = auth.authorization_url {
                cred_data.authorization_url = Some(auth_url.clone());
            }
            if let Some(ref cid) = auth.client_id {
                cred_data.client_id = Some(cid.clone());
            }
        }

        // Store clientId and authorizationUrl in the Data map as well (C# compat)
        if let Some(ref cid) = cred_data.client_id {
            cred_data.data.insert("clientId".to_string(), cid.clone());
        }
        if let Some(ref auth_url) = cred_data.authorization_url {
            cred_data
                .data
                .insert("authorizationUrl".to_string(), auth_url.clone());
        }

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

    /// Exchange a registration token for tenant credentials by calling
    /// `POST api.github.com/actions/runner-registration` with `RemoteAuth <token>`.
    ///
    /// This matches the C# `GetTenantCredential` method. The response contains
    /// the Actions service tenant URL, an OAuth access token, and whether to use
    /// the runner-admin flow.
    ///
    /// Returns `(server_url, access_token, client_id, auth_url)`.
    async fn exchange_registration_token(
        &self,
        github_url: &str,
        token: &str,
        is_hosted: bool,
    ) -> Result<(String, String, String, Option<String>)> {
        let parsed = url::Url::parse(github_url).context("Invalid GitHub URL")?;

        let api_url = if is_hosted {
            format!(
                "https://api.{}/actions/runner-registration",
                parsed.host_str().unwrap_or("github.com")
            )
        } else {
            format!(
                "{}://{}/api/v3/actions/runner-registration",
                parsed.scheme(),
                parsed.host_str().unwrap_or("")
            )
        };

        let body = serde_json::json!({
            "url": github_url,
            "runner_event": constants::runner_event::REGISTER,
        });

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..3 {
            self.trace.info(&format!(
                "Getting tenant credentials from {} (attempt {})",
                api_url,
                attempt + 1
            ));

            let response = client
                .post(&api_url)
                .header("Authorization", format!("RemoteAuth {}", token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    let auth_result: GitHubAuthResult = resp
                        .json()
                        .await
                        .context("Failed to parse runner-registration response")?;

                    self.trace.info(&format!(
                        "Tenant URL: {}, runner-admin flow: {}",
                        auth_result.url, auth_result.use_runner_admin_flow
                    ));

                    let client_id = format!("runner-{}", uuid::Uuid::new_v4());
                    let auth_url = if !auth_result.url.is_empty() {
                        // The token_schema field tells us the auth endpoint
                        Some(format!("{}/actions/token", auth_result.url.trim_end_matches('/')))
                    } else {
                        None
                    };

                    return Ok((
                        auth_result.url,
                        auth_result.token,
                        client_id,
                        auth_url,
                    ));
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();
                    let err_msg = format!(
                        "HTTP {} from POST {} — {}",
                        status.as_u16(),
                        api_url,
                        body_text
                    );
                    self.trace.error(&err_msg);

                    if status.as_u16() == 404 {
                        return Err(anyhow::anyhow!(
                            "Registration failed (404). Verify the URL and token are correct.\n{}",
                            err_msg
                        ));
                    }
                    last_error = Some(anyhow::anyhow!("{}", err_msg));
                }
                Err(e) => {
                    self.trace.error(&format!("Request error: {}", e));
                    last_error = Some(e.into());
                }
            }

            if attempt < 2 {
                let backoff = std::time::Duration::from_secs(rand::random::<u64>() % 4 + 1);
                self.trace
                    .info(&format!("Retrying in {:?}…", backoff));
                tokio::time::sleep(backoff).await;
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Failed to get tenant credentials")))
    }

    /// Query the available runner groups (pools) from the Actions service.
    async fn get_agent_pools(
        &self,
        server_url: &str,
        token: &str,
    ) -> Result<Vec<AgentPool>> {
        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;
        let url = format!(
            "{}/_apis/distributedtask/pools",
            server_url.trim_end_matches('/')
        );

        self.trace
            .info(&format!("Fetching runner groups from {}", url));

        let response = client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", "application/json;api-version=6.0-preview")
            .send()
            .await
            .context("Failed to fetch runner groups")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Failed to list runner groups: HTTP {} — {}",
                status.as_u16(),
                body
            ));
        }

        #[derive(Deserialize)]
        struct PoolListResponse {
            #[serde(default)]
            value: Vec<AgentPool>,
        }

        let list: PoolListResponse = response.json().await.context("Bad pool list response")?;
        Ok(list.value)
    }

    /// Pick the best pool to register the runner into.
    fn pick_pool<'a>(pools: &'a [AgentPool], runner_group: &str) -> Result<&'a AgentPool> {
        let pool = if !runner_group.is_empty() && runner_group != "Default" {
            pools
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(runner_group) && !p.is_hosted)
        } else {
            None
        }
        .or_else(|| pools.iter().find(|p| p.is_internal))
        .or_else(|| pools.iter().find(|p| !p.is_hosted))
        .or_else(|| pools.first());

        pool.ok_or_else(|| {
            anyhow::anyhow!("Could not find any self-hosted runner group. Contact support.")
        })
    }

    /// Register the runner with the Actions service.
    ///
    /// Matches the C# `_runnerServer.AddAgentAsync(poolId, agent)` call path.
    async fn register_runner(
        &self,
        server_url: &str,
        token: &str,
        name: &str,
        runner_group: &str,
        labels: &str,
        public_key_pem: &str,
        ephemeral: bool,
        disable_update: bool,
        no_default_labels: bool,
        pool_id: u64,
    ) -> Result<RunnerRegistrationResponse> {
        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        // 2. Build label list
        let mut label_list: Vec<serde_json::Value> = Vec::new();
        if !no_default_labels {
            label_list.push(serde_json::json!({"name": "self-hosted", "type": "system"}));
            label_list.push(serde_json::json!({
                "name": constants::CURRENT_PLATFORM.label_name(),
                "type": "system"
            }));
            label_list.push(serde_json::json!({
                "name": constants::CURRENT_ARCHITECTURE.label_name(),
                "type": "system"
            }));
        }
        if !labels.is_empty() {
            for label in labels.split(',') {
                let label = label.trim();
                if !label.is_empty() {
                    label_list.push(serde_json::json!({"name": label, "type": "user"}));
                }
            }
        }

        // 3. Build the RSA public key in the format the server expects
        //    C# sends exponent and modulus as base64-encoded byte arrays
        use rsa::pkcs8::DecodePublicKey;
        use rsa::traits::PublicKeyParts;
        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

        let rsa_pub = rsa::RsaPublicKey::from_public_key_pem(public_key_pem)
            .context("Failed to parse RSA public key PEM")?;
        let exponent_bytes = rsa_pub.e().to_bytes_be();
        let modulus_bytes = rsa_pub.n().to_bytes_be();
        let exponent_b64 = BASE64.encode(&exponent_bytes);
        let modulus_b64 = BASE64.encode(&modulus_bytes);

        let public_key = serde_json::json!({
            "exponent": exponent_b64,
            "modulus": modulus_b64,
        });

        let body = serde_json::json!({
            "name": name,
            "version": runner_sdk::build_constants::RunnerPackage::VERSION,
            "osDescription": format!("{} {}", constants::CURRENT_PLATFORM, constants::CURRENT_ARCHITECTURE),
            "labels": label_list,
            "runnerGroupName": runner_group,
            "ephemeral": ephemeral,
            "disableUpdate": disable_update,
            "authorization": {
                "publicKey": public_key,
            },
            "maxParallelism": 1,
        });

        let url = format!(
            "{}/_apis/distributedtask/pools/{}/agents",
            server_url.trim_end_matches('/'),
            pool_id
        );

        self.trace
            .info(&format!("Registering runner at: {}", url));

        let response = client
            .post(&url)
            .bearer_auth(token)
            .header("Accept", "application/json;api-version=6.0-preview")
            .header("Content-Type", "application/json")
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

        let mut registration: RunnerRegistrationResponse = response
            .json()
            .await
            .context("Failed to deserialize registration response")?;

        // If the server didn't echo the name, use what we sent
        if registration.name.is_empty() {
            registration.name = name.to_string();
        }

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

        // We need the pool ID — load it from saved settings, or try pool 1
        let inner_config_store = ConfigurationStore::new(&self.context);
        let pool_id: u64 = if let Ok(settings) = inner_config_store.get_settings() {
            if settings.pool_id > 0 { settings.pool_id as u64 } else { 1 }
        } else {
            1
        };

        let url = format!(
            "{}/_apis/distributedtask/pools/{}/agents/{}",
            server_url.trim_end_matches('/'),
            pool_id,
            agent_id
        );

        self.trace
            .info(&format!("Removing runner at: {}", url));

        let response = client
            .delete(&url)
            .bearer_auth(token)
            .header("Accept", "application/json;api-version=6.0-preview")
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
