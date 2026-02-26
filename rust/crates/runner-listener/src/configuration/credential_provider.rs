// CredentialProvider mapping credential providers from the C# runner.
// PersonalAccessToken and OAuthCredential implementations.

use anyhow::{Context, Result};
use async_trait::async_trait;
use runner_common::credential_data::CredentialData;
use runner_common::host_context::HostContext;
use serde::Deserialize;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A credential provider that can supply access tokens for API calls.
///
/// Maps the C# `ICredentialProvider` interface.
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    /// Get an access token for authenticating with the Actions service.
    async fn get_token(&self) -> Result<String>;

    /// Get the scheme name for this provider.
    fn scheme(&self) -> &str;

    /// Get the credential data backing this provider.
    fn credential_data(&self) -> &CredentialData;
}

// ---------------------------------------------------------------------------
// PersonalAccessToken (PAT) provider
// ---------------------------------------------------------------------------

/// A credential provider that uses a stored personal access token or
/// pre-existing OAuth access token directly.
pub struct PatCredentialProvider {
    credential: CredentialData,
}

impl PatCredentialProvider {
    /// Create a new PAT credential provider.
    pub fn new(credential: CredentialData) -> Self {
        Self { credential }
    }
}

#[async_trait]
impl CredentialProvider for PatCredentialProvider {
    async fn get_token(&self) -> Result<String> {
        // Look for the token in the data map
        if let Some(token) = self.credential.get_data("accessToken") {
            return Ok(token.clone());
        }
        if let Some(token) = self.credential.get_data("token") {
            return Ok(token.clone());
        }
        Err(anyhow::anyhow!(
            "No access token found in PAT credential data"
        ))
    }

    fn scheme(&self) -> &str {
        &self.credential.scheme
    }

    fn credential_data(&self) -> &CredentialData {
        &self.credential
    }
}

// ---------------------------------------------------------------------------
// OAuth credential provider
// ---------------------------------------------------------------------------

/// A credential provider that exchanges a JWT for an OAuth access token
/// using an RSA key pair stored on disk.
pub struct OAuthCredentialProvider {
    context: Arc<HostContext>,
    credential: CredentialData,
}

impl OAuthCredentialProvider {
    /// Create a new OAuth credential provider.
    pub fn new(context: Arc<HostContext>, credential: CredentialData) -> Self {
        Self {
            context,
            credential,
        }
    }
}

#[async_trait]
impl CredentialProvider for OAuthCredentialProvider {
    async fn get_token(&self) -> Result<String> {
        // If we already have an access token cached, use it
        if let Some(token) = self.credential.get_data("accessToken") {
            if !token.is_empty() {
                return Ok(token.clone());
            }
        }

        // Otherwise, perform the OAuth JWT exchange
        let auth_url = self
            .credential
            .authorization_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("No authorization URL in OAuth credential data"))?;

        let client_id = self
            .credential
            .client_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("No client ID in OAuth credential data"))?;

        // Read the RSA private key
        let rsa_key_path = self
            .context
            .get_config_file(runner_common::constants::WellKnownConfigFile::RSACredentials);

        let rsa_pem = std::fs::read_to_string(&rsa_key_path)
            .context("Failed to read RSA key for OAuth")?;

        // Build JWT claims
        let now = chrono::Utc::now();
        let claims = serde_json::json!({
            "sub": client_id,
            "iss": client_id,
            "aud": auth_url,
            "nbf": (now - chrono::Duration::minutes(5)).timestamp(),
            "exp": (now + chrono::Duration::minutes(5)).timestamp(),
        });

        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(rsa_pem.as_bytes())
            .context("Failed to parse RSA key for OAuth")?;

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
            .context("Failed to encode JWT for OAuth")?;

        // Exchange the JWT for an access token
        let client =
            runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let response = client
            .post(auth_url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .context("OAuth token exchange request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "OAuth token exchange failed with HTTP {}: {}",
                status.as_u16(),
                body
            ));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
        }

        let token_resp: TokenResponse = response
            .json()
            .await
            .context("Failed to deserialize OAuth token response")?;

        Ok(token_resp.access_token)
    }

    fn scheme(&self) -> &str {
        &self.credential.scheme
    }

    fn credential_data(&self) -> &CredentialData {
        &self.credential
    }
}
