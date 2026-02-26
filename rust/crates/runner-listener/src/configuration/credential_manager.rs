// CredentialManager mapping `CredentialManager.cs`.
// Factory for creating credential providers based on the credential scheme.

use anyhow::Result;
use runner_common::credential_data::CredentialData;
use runner_common::host_context::HostContext;
use runner_sdk::TraceWriter;
use std::sync::Arc;

use super::credential_provider::{CredentialProvider, OAuthCredentialProvider, PatCredentialProvider};

/// Factory for creating credential providers based on the stored credential data.
///
/// Maps `CredentialManager` in the C# runner.
pub struct CredentialManager {
    context: Arc<HostContext>,
}

impl CredentialManager {
    /// Create a new `CredentialManager`.
    pub fn new(context: Arc<HostContext>) -> Self {
        Self { context }
    }

    /// Create a credential provider from stored credential data.
    ///
    /// Inspects the `scheme` field to determine which provider to use:
    /// - `"OAuth"` → `OAuthCredentialProvider`
    /// - `"OAuthAccessToken"` / `"PersonalAccessToken"` → `PatCredentialProvider`
    pub fn create_provider(
        &self,
        credential: &CredentialData,
    ) -> Result<Box<dyn CredentialProvider>> {
        let trace = self.context.get_trace("CredentialManager");

        match credential.scheme.as_str() {
            "OAuth" => {
                trace.info("Creating OAuth credential provider");
                let provider = OAuthCredentialProvider::new(
                    self.context.clone(),
                    credential.clone(),
                );
                Ok(Box::new(provider))
            }
            "OAuthAccessToken" | "PersonalAccessToken" | "PAT" => {
                trace.info("Creating PAT credential provider");
                let provider = PatCredentialProvider::new(credential.clone());
                Ok(Box::new(provider))
            }
            scheme => {
                Err(anyhow::anyhow!(
                    "Unknown credential scheme: '{}'. Expected 'OAuth' or 'PersonalAccessToken'.",
                    scheme
                ))
            }
        }
    }

    /// Load credentials from the config store and create a provider.
    pub fn load_and_create_provider(&self) -> Result<Box<dyn CredentialProvider>> {
        let config_store = runner_common::config_store::ConfigurationStore::new(&self.context);
        let creds = config_store.get_credentials()?;
        self.create_provider(&creds)
    }
}
