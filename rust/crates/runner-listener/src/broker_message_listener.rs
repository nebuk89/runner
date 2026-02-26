// BrokerMessageListener mapping `BrokerMessageListener.cs` (V2 broker-based listener).
// Same interface as V1 MessageListener but connects via the BrokerServer URL.
// Used when `RunnerSettings.use_v2_flow` is true (migrated runner).

use anyhow::{Context, Result};
use runner_common::config_store::{ConfigurationStore, RunnerSettings};
use runner_common::credential_data::CredentialData;
use runner_common::host_context::HostContext;
use runner_sdk::TraceWriter;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Maximum retries when creating a broker session.
const MAX_SESSION_CREATE_RETRIES: u32 = 30;

/// Delay between broker session creation retries.
const SESSION_CREATE_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Long-poll timeout for getting next message from the broker.
const GET_MESSAGE_TIMEOUT: Duration = Duration::from_secs(50);

// ---------------------------------------------------------------------------
// Broker-specific types
// ---------------------------------------------------------------------------

/// A session created on the broker server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(default, rename = "runnerToken")]
    pub runner_token: Option<String>,
    #[serde(default, rename = "encryptionKey")]
    pub encryption_key: Option<String>,
}

/// A message received from the broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerMessage {
    #[serde(rename = "messageId")]
    pub message_id: u64,
    #[serde(rename = "messageType")]
    pub message_type: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, rename = "iv")]
    pub initialization_vector: Option<String>,
}

impl BrokerMessage {
    /// Returns the message type as a well-known variant.
    pub fn type_kind(&self) -> BrokerMessageType {
        match self.message_type.as_str() {
            "RunnerJobRequest" => BrokerMessageType::RunnerJobRequest,
            "RunnerRefreshMessage" => BrokerMessageType::RunnerRefresh,
            "JobCancelMessage" => BrokerMessageType::JobCancel,
            "ForceTokenRefreshMessage" => BrokerMessageType::ForceTokenRefresh,
            "RunnerRefreshConfig" => BrokerMessageType::RunnerRefreshConfig,
            "HostedRunnerShutdown" => BrokerMessageType::HostedRunnerShutdown,
            _ => BrokerMessageType::Unknown,
        }
    }
}

/// Well-known broker message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerMessageType {
    RunnerJobRequest,
    RunnerRefresh,
    JobCancel,
    ForceTokenRefresh,
    RunnerRefreshConfig,
    HostedRunnerShutdown,
    Unknown,
}

// ---------------------------------------------------------------------------
// BrokerMessageListener
// ---------------------------------------------------------------------------

/// V2 broker message listener.
///
/// Uses the broker URL from `RunnerSettings.server_url_v2` to create sessions
/// and poll for messages. This replaces the V1 MessageListener for migrated runners.
pub struct BrokerMessageListener {
    context: Arc<HostContext>,
    trace: runner_common::tracing::Tracing,
    session: Option<BrokerSession>,
    settings: Option<RunnerSettings>,
    credentials: Option<CredentialData>,
    last_message_id: u64,
    access_token: Option<String>,
}

impl BrokerMessageListener {
    /// Create a new `BrokerMessageListener`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("BrokerMessageListener");
        Self {
            context,
            trace,
            session: None,
            settings: None,
            credentials: None,
            last_message_id: 0,
            access_token: None,
        }
    }

    /// Create a session on the broker server.
    pub async fn create_session_async(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<()> {
        let config_store = ConfigurationStore::new(&self.context);

        // Try migrated settings first, then regular settings
        let settings = config_store
            .get_migrated_settings()
            .or_else(|_| config_store.get_settings())
            .context("Failed to load runner settings for broker session creation")?;

        let credentials = config_store
            .get_migrated_credentials()
            .or_else(|_| config_store.get_credentials())
            .context("Failed to load credentials for broker session creation")?;

        let broker_url = settings
            .server_url_v2
            .as_deref()
            .unwrap_or(&settings.server_url);

        self.trace.info(&format!(
            "Creating broker session for runner '{}' at {}",
            settings.agent_name, broker_url
        ));

        self.settings = Some(settings.clone());
        self.credentials = Some(credentials.clone());

        let mut retry_count = 0u32;

        loop {
            if cancel.is_cancelled() {
                return Err(anyhow::anyhow!("Broker session creation cancelled"));
            }

            match self.try_create_broker_session(&settings, &credentials, broker_url).await {
                Ok(session) => {
                    self.trace.info(&format!(
                        "Broker session created: {}",
                        session.session_id
                    ));
                    if let Some(ref token) = session.runner_token {
                        self.access_token = Some(token.clone());
                    }
                    self.session = Some(session);
                    return Ok(());
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= MAX_SESSION_CREATE_RETRIES {
                        return Err(e).context(format!(
                            "Failed to create broker session after {} retries",
                            MAX_SESSION_CREATE_RETRIES
                        ));
                    }

                    self.trace.warning(&format!(
                        "Failed to create broker session (attempt {}/{}): {}. Retrying in {}s...",
                        retry_count,
                        MAX_SESSION_CREATE_RETRIES,
                        e,
                        SESSION_CREATE_RETRY_DELAY.as_secs()
                    ));

                    tokio::select! {
                        _ = tokio::time::sleep(SESSION_CREATE_RETRY_DELAY) => {},
                        _ = cancel.cancelled() => {
                            return Err(anyhow::anyhow!("Broker session creation cancelled during retry delay"));
                        }
                    }
                }
            }
        }
    }

    /// Try to create a broker session once.
    async fn try_create_broker_session(
        &mut self,
        settings: &RunnerSettings,
        credentials: &CredentialData,
        broker_url: &str,
    ) -> Result<BrokerSession> {
        let token = self.obtain_access_token(credentials).await?;
        self.access_token = Some(token.clone());

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/v1/runners/{}/session",
            broker_url, settings.agent_id
        );

        let session_request = serde_json::json!({
            "runnerId": settings.agent_id,
            "runnerName": settings.agent_name,
        });

        let response = client
            .post(&url)
            .bearer_auth(&token)
            .json(&session_request)
            .send()
            .await
            .context("Failed to send broker session create request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Broker session create failed with HTTP {}: {}",
                status.as_u16(),
                body
            ));
        }

        let session: BrokerSession = response
            .json()
            .await
            .context("Failed to deserialize broker session response")?;

        Ok(session)
    }

    /// Get the next message from the broker via long-poll.
    pub async fn get_next_message_async(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<Option<BrokerMessage>> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active broker session"))?;

        let settings = self
            .settings
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No settings loaded"))?;

        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token available"))?;

        let broker_url = settings
            .server_url_v2
            .as_deref()
            .unwrap_or(&settings.server_url);

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/v1/runners/{}/session/{}/message?lastMessageId={}",
            broker_url, settings.agent_id, session.session_id, self.last_message_id
        );

        let response = tokio::select! {
            result = async {
                client
                    .get(&url)
                    .bearer_auth(token)
                    .timeout(GET_MESSAGE_TIMEOUT)
                    .send()
                    .await
            } => result.context("Failed to poll broker for messages")?,
            _ = cancel.cancelled() => {
                return Ok(None);
            }
        };

        let status = response.status();

        if status == reqwest::StatusCode::ACCEPTED || status == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            self.trace
                .warning("Got 401/403 from broker — refreshing access token");
            if let Some(creds) = &self.credentials.clone() {
                if let Ok(new_token) = self.obtain_access_token(creds).await {
                    self.access_token = Some(new_token);
                }
            }
            return Ok(None);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Broker get message failed with HTTP {}: {}",
                status.as_u16(),
                body
            ));
        }

        let message: BrokerMessage = response
            .json()
            .await
            .context("Failed to deserialize broker message")?;

        self.last_message_id = message.message_id;

        self.trace.info(&format!(
            "Received broker message #{}: type={}",
            message.message_id, message.message_type
        ));

        Ok(Some(message))
    }

    /// Delete the broker session on the server.
    pub async fn delete_session_async(&mut self) -> Result<()> {
        let session = match self.session.take() {
            Some(s) => s,
            None => return Ok(()),
        };

        let settings = match &self.settings {
            Some(s) => s,
            None => return Ok(()),
        };

        let token = match &self.access_token {
            Some(t) => t.clone(),
            None => return Ok(()),
        };

        let broker_url = settings
            .server_url_v2
            .as_deref()
            .unwrap_or(&settings.server_url);

        self.trace.info(&format!(
            "Deleting broker session {}",
            session.session_id
        ));

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/v1/runners/{}/session/{}",
            broker_url, settings.agent_id, session.session_id
        );

        let _ = client.delete(&url).bearer_auth(&token).send().await;

        Ok(())
    }

    /// Delete a processed message from the broker.
    pub async fn delete_message_async(&self, message: &BrokerMessage) -> Result<()> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active broker session"))?;

        let settings = self
            .settings
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No settings loaded"))?;

        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token"))?;

        let broker_url = settings
            .server_url_v2
            .as_deref()
            .unwrap_or(&settings.server_url);

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/v1/runners/{}/session/{}/message/{}",
            broker_url, settings.agent_id, session.session_id, message.message_id
        );

        let _ = client.delete(&url).bearer_auth(token).send().await;

        Ok(())
    }

    /// Obtain an access token from the credential data.
    async fn obtain_access_token(&self, credentials: &CredentialData) -> Result<String> {
        if let Some(token) = credentials.get_data("accessToken") {
            return Ok(token.clone());
        }

        if let Some(token) = credentials.get_data("token") {
            return Ok(token.clone());
        }

        // If we have an OAuth flow, exchange credentials
        if let (Some(client_id), Some(auth_url)) =
            (&credentials.client_id, &credentials.authorization_url)
        {
            return self.exchange_oauth_token(client_id, auth_url).await;
        }

        Err(anyhow::anyhow!(
            "No access token or credential data available for broker"
        ))
    }

    /// Exchange an OAuth token using client credentials (same mechanism as V1).
    async fn exchange_oauth_token(
        &self,
        client_id: &str,
        auth_url: &str,
    ) -> Result<String> {
        let rsa_key_path = self
            .context
            .get_config_file(runner_common::constants::WellKnownConfigFile::RSACredentials);

        let rsa_pem = std::fs::read_to_string(&rsa_key_path)
            .context("Failed to read RSA key for broker OAuth token exchange")?;

        let now = chrono::Utc::now();
        let claims = serde_json::json!({
            "sub": client_id,
            "iss": client_id,
            "aud": auth_url,
            "nbf": (now - chrono::Duration::minutes(5)).timestamp(),
            "exp": (now + chrono::Duration::minutes(5)).timestamp(),
        });

        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(rsa_pem.as_bytes())
            .context("Failed to parse RSA key for broker")?;

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
            .context("Failed to encode JWT for broker")?;

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let response = client
            .post(auth_url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .context("Broker OAuth token exchange request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Broker OAuth token exchange failed with HTTP {}: {}",
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
            .context("Failed to deserialize broker OAuth token response")?;

        Ok(token_resp.access_token)
    }

    /// Get the current session ID, if any.
    pub fn session_id(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.session_id.as_str())
    }

    /// Update the access token (e.g., after a ForceTokenRefresh message).
    pub fn set_access_token(&mut self, token: String) {
        self.access_token = Some(token);
    }
}
