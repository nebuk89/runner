// ConfigurationStore mapping `ConfigurationStore.cs`.
// Handles loading/saving runner settings and credentials from disk.

use crate::constants::{WellKnownConfigFile, WellKnownDirectory};
use crate::credential_data::CredentialData;
use crate::host_context::HostContext;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// RunnerSettings
// ---------------------------------------------------------------------------

/// Persisted runner configuration, mapping `RunnerSettings` in C#.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunnerSettings {
    /// The runner's unique agent ID.
    #[serde(default, rename = "AgentId")]
    pub agent_id: u64,

    /// The runner's display name.
    #[serde(default, rename = "AgentName")]
    pub agent_name: String,

    /// Whether to skip session recovery on startup.
    #[serde(default, rename = "SkipSessionRecover")]
    pub skip_session_recover: bool,

    /// The pool / runner group ID.
    #[serde(default, rename = "PoolId")]
    pub pool_id: i32,

    /// The pool / runner group name.
    #[serde(default, rename = "PoolName")]
    pub pool_name: String,

    /// Whether auto-update is disabled.
    #[serde(default, rename = "DisableUpdate")]
    pub disable_update: bool,

    /// Whether the runner is ephemeral (single-use).
    #[serde(default, rename = "Ephemeral")]
    pub is_ephemeral: bool,

    /// The Actions service URL.
    #[serde(default, rename = "ServerUrl")]
    pub server_url: String,

    /// The GitHub URL (e.g. `https://github.com/owner/repo`).
    #[serde(default, rename = "GitHubUrl")]
    pub git_hub_url: String,

    /// The work directory name / path (relative to root).
    #[serde(default, rename = "WorkFolder")]
    pub work_folder: String,

    /// Monitor socket address for the supervisor process.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "MonitorSocketAddress")]
    pub monitor_socket_address: Option<String>,

    /// Whether to use the v2 listener flow.
    #[serde(default, rename = "UseV2Flow")]
    pub use_v2_flow: bool,

    /// Whether to use the runner admin flow.
    #[serde(default, rename = "UseRunnerAdminFlow")]
    pub use_runner_admin_flow: bool,

    /// The v2 service URL.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ServerUrlV2")]
    pub server_url_v2: Option<String>,

    /// Cached value of whether this is a hosted (github.com) server.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "IsHostedServer"
    )]
    is_hosted_server: Option<bool>,
}

impl RunnerSettings {
    /// Returns whether this runner is configured against a hosted (github.com / ghe.com) server.
    ///
    /// The logic mirrors the C# property with fallback inference from URLs.
    pub fn is_hosted_server(&self) -> bool {
        // Explicitly set
        if let Some(v) = self.is_hosted_server {
            return v;
        }

        // Infer from GitHubUrl
        if !self.git_hub_url.is_empty() {
            if let Ok(url) = url::Url::parse(&self.git_hub_url) {
                return runner_sdk::UrlUtil::is_hosted_server(&url);
            }
        }

        // Env override: force GHES
        if let Ok(val) = std::env::var("GITHUB_ACTIONS_RUNNER_FORCE_GHES") {
            if runner_sdk::StringUtil::convert_to_bool(&val) == Some(true) {
                return false;
            }
        }

        // Env override: force empty GitHub URL is hosted
        if self.git_hub_url.is_empty() {
            if let Ok(val) = std::env::var("GITHUB_ACTIONS_RUNNER_FORCE_EMPTY_GITHUB_URL_IS_HOSTED")
            {
                if runner_sdk::StringUtil::convert_to_bool(&val) == Some(true) {
                    return true;
                }
            }
        }

        // Infer from ServerUrl
        if !self.server_url.is_empty() {
            if let Ok(url) = url::Url::parse(&self.server_url) {
                if let Some(host) = url.host_str() {
                    let host = host.to_lowercase();
                    return host.ends_with(".actions.githubusercontent.com")
                        || host.ends_with(".codedev.ms");
                }
            }
        }

        // Infer from ServerUrlV2
        if let Some(ref server_url_v2) = self.server_url_v2 {
            if !server_url_v2.is_empty() {
                if let Ok(url) = url::Url::parse(server_url_v2) {
                    if let Some(host) = url.host_str() {
                        let host = host.to_lowercase();
                        return host.ends_with(".actions.githubusercontent.com")
                            || host.ends_with(".githubapp.com")
                            || host.ends_with(".ghe.com")
                            || host.ends_with(".actions.localhost")
                            || host.ends_with(".ghe.localhost");
                    }
                }
            }
        }

        // Default to true
        true
    }

    /// Set the hosted server flag explicitly.
    pub fn set_is_hosted_server(&mut self, value: bool) {
        self.is_hosted_server = Some(value);
    }

    /// Computed property that returns the repo or org name from the server/GitHub URLs.
    pub fn repo_or_org_name(&self) -> String {
        if !self.git_hub_url.is_empty() {
            if let Ok(github_url) = url::Url::parse(&self.git_hub_url) {
                if let Ok(server_url) = url::Url::parse(&self.server_url) {
                    if let Some(host) = server_url.host_str() {
                        if host.ends_with(".githubusercontent.com") {
                            let path = github_url.path().trim_matches('/');
                            if !path.is_empty() {
                                return path.to_string();
                            }
                        }
                    }
                }
            }
        }

        // Fallback: first path segment of server URL
        if let Ok(url) = url::Url::parse(&self.server_url) {
            let segments: Vec<&str> = url
                .path_segments()
                .map(|s| s.collect())
                .unwrap_or_default();
            if let Some(first) = segments.first() {
                if !first.is_empty() {
                    return first.to_string();
                }
            }
        }

        String::new()
    }
}

// ---------------------------------------------------------------------------
// ConfigurationStore
// ---------------------------------------------------------------------------

/// Handles loading and saving runner settings and credentials.
///
/// Maps `ConfigurationStore` in the C# runner.
pub struct ConfigurationStore {
    config_file_path: PathBuf,
    migrated_config_file_path: PathBuf,
    cred_file_path: PathBuf,
    migrated_cred_file_path: PathBuf,
    service_config_file_path: PathBuf,
    root_folder: PathBuf,

    settings: Mutex<Option<RunnerSettings>>,
    migrated_settings: Mutex<Option<RunnerSettings>>,
    creds: Mutex<Option<CredentialData>>,
    migrated_creds: Mutex<Option<CredentialData>>,
}

impl ConfigurationStore {
    /// Create a new `ConfigurationStore` initialized from the host context.
    pub fn new(context: &Arc<HostContext>) -> Self {
        let root = context.get_directory(WellKnownDirectory::Root);

        Self {
            config_file_path: context.get_config_file(WellKnownConfigFile::Runner),
            migrated_config_file_path: context.get_config_file(WellKnownConfigFile::MigratedRunner),
            cred_file_path: context.get_config_file(WellKnownConfigFile::Credentials),
            migrated_cred_file_path: context
                .get_config_file(WellKnownConfigFile::MigratedCredentials),
            service_config_file_path: context.get_config_file(WellKnownConfigFile::Service),
            root_folder: root,
            settings: Mutex::new(None),
            migrated_settings: Mutex::new(None),
            creds: Mutex::new(None),
            migrated_creds: Mutex::new(None),
        }
    }

    /// Returns the root folder of the runner installation.
    pub fn root_folder(&self) -> &PathBuf {
        &self.root_folder
    }

    /// Check whether the runner has been configured (settings file exists).
    pub fn is_configured(&self) -> bool {
        self.config_file_path.exists() || self.migrated_config_file_path.exists()
    }

    /// Check whether the service config file exists.
    pub fn is_service_configured(&self) -> bool {
        self.service_config_file_path.exists()
    }

    /// Check whether migrated settings exist.
    pub fn is_migrated_configured(&self) -> bool {
        self.migrated_config_file_path.exists()
    }

    /// Check whether credentials are stored on disk.
    pub fn has_credentials(&self) -> bool {
        self.cred_file_path.exists() || self.migrated_cred_file_path.exists()
    }

    /// Load and return runner settings. Cached after first load.
    pub fn get_settings(&self) -> Result<RunnerSettings> {
        let mut guard = self.settings.lock().unwrap();
        if let Some(ref settings) = *guard {
            return Ok(settings.clone());
        }

        let json = fs::read_to_string(&self.config_file_path)
            .with_context(|| format!("Failed to read settings from {:?}", self.config_file_path))?;

        let settings: RunnerSettings = serde_json::from_str(&json)
            .with_context(|| "Failed to deserialize runner settings")?;

        *guard = Some(settings.clone());
        Ok(settings)
    }

    /// Load migrated runner settings.
    pub fn get_migrated_settings(&self) -> Result<RunnerSettings> {
        let mut guard = self.migrated_settings.lock().unwrap();
        if let Some(ref settings) = *guard {
            return Ok(settings.clone());
        }

        let json = fs::read_to_string(&self.migrated_config_file_path).with_context(|| {
            format!(
                "Failed to read migrated settings from {:?}",
                self.migrated_config_file_path
            )
        })?;

        let settings: RunnerSettings = serde_json::from_str(&json)
            .with_context(|| "Failed to deserialize migrated runner settings")?;

        *guard = Some(settings.clone());
        Ok(settings)
    }

    /// Load and return credentials. Cached after first load.
    pub fn get_credentials(&self) -> Result<CredentialData> {
        let mut guard = self.creds.lock().unwrap();
        if let Some(ref creds) = *guard {
            return Ok(creds.clone());
        }

        let json = fs::read_to_string(&self.cred_file_path)
            .with_context(|| format!("Failed to read credentials from {:?}", self.cred_file_path))?;

        let creds: CredentialData = serde_json::from_str(&json)
            .with_context(|| "Failed to deserialize credential data")?;

        *guard = Some(creds.clone());
        Ok(creds)
    }

    /// Load migrated credentials.
    pub fn get_migrated_credentials(&self) -> Result<CredentialData> {
        let mut guard = self.migrated_creds.lock().unwrap();
        if let Some(ref creds) = *guard {
            return Ok(creds.clone());
        }

        if !self.migrated_cred_file_path.exists() {
            anyhow::bail!("Migrated credentials file does not exist");
        }

        let json = fs::read_to_string(&self.migrated_cred_file_path).with_context(|| {
            format!(
                "Failed to read migrated credentials from {:?}",
                self.migrated_cred_file_path
            )
        })?;

        let creds: CredentialData = serde_json::from_str(&json)
            .with_context(|| "Failed to deserialize migrated credential data")?;

        *guard = Some(creds.clone());
        Ok(creds)
    }

    /// Save runner settings to disk.
    pub fn save_settings(&self, settings: &RunnerSettings) -> Result<()> {
        // Delete existing file first (mirrors C# behavior for hidden files)
        if self.config_file_path.exists() {
            fs::remove_file(&self.config_file_path)?;
        }

        let json = serde_json::to_string_pretty(settings)?;
        fs::write(&self.config_file_path, &json)
            .with_context(|| format!("Failed to write settings to {:?}", self.config_file_path))?;

        // Update cache
        *self.settings.lock().unwrap() = Some(settings.clone());
        Ok(())
    }

    /// Save migrated runner settings to disk.
    pub fn save_migrated_settings(&self, settings: &RunnerSettings) -> Result<()> {
        if self.migrated_config_file_path.exists() {
            fs::remove_file(&self.migrated_config_file_path)?;
        }

        let json = serde_json::to_string_pretty(settings)?;
        fs::write(&self.migrated_config_file_path, &json).with_context(|| {
            format!(
                "Failed to write migrated settings to {:?}",
                self.migrated_config_file_path
            )
        })?;

        *self.migrated_settings.lock().unwrap() = Some(settings.clone());
        Ok(())
    }

    /// Save credentials to disk.
    pub fn save_credential(&self, credential: &CredentialData) -> Result<()> {
        if self.cred_file_path.exists() {
            fs::remove_file(&self.cred_file_path)?;
        }

        let json = serde_json::to_string_pretty(credential)?;
        fs::write(&self.cred_file_path, &json).with_context(|| {
            format!(
                "Failed to write credentials to {:?}",
                self.cred_file_path
            )
        })?;

        *self.creds.lock().unwrap() = Some(credential.clone());
        Ok(())
    }

    /// Save migrated credentials to disk.
    pub fn save_migrated_credential(&self, credential: &CredentialData) -> Result<()> {
        if self.migrated_cred_file_path.exists() {
            fs::remove_file(&self.migrated_cred_file_path)?;
        }

        let json = serde_json::to_string_pretty(credential)?;
        fs::write(&self.migrated_cred_file_path, &json).with_context(|| {
            format!(
                "Failed to write migrated credentials to {:?}",
                self.migrated_cred_file_path
            )
        })?;

        *self.migrated_creds.lock().unwrap() = Some(credential.clone());
        Ok(())
    }

    /// Delete credentials (both primary and migrated).
    pub fn delete_credential(&self) {
        let _ = fs::remove_file(&self.cred_file_path);
        let _ = fs::remove_file(&self.migrated_cred_file_path);
        *self.creds.lock().unwrap() = None;
        *self.migrated_creds.lock().unwrap() = None;
    }

    /// Delete migrated credentials only.
    pub fn delete_migrated_credential(&self) {
        let _ = fs::remove_file(&self.migrated_cred_file_path);
        *self.migrated_creds.lock().unwrap() = None;
    }

    /// Delete settings (both primary and migrated).
    pub fn delete_settings(&self) {
        let _ = fs::remove_file(&self.config_file_path);
        let _ = fs::remove_file(&self.migrated_config_file_path);
        *self.settings.lock().unwrap() = None;
        *self.migrated_settings.lock().unwrap() = None;
    }

    /// Check whether the given settings point to a hosted (github.com) server.
    pub fn is_hosted_server(settings: &RunnerSettings) -> bool {
        settings.is_hosted_server()
    }
}
