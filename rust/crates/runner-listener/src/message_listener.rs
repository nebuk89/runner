// MessageListener mapping `MessageListener.cs` (V1 legacy listener).
// Creates sessions, polls for messages, handles OAuth refresh, session conflicts, clock skew.

use anyhow::{Context, Result};
use runner_common::config_store::{ConfigurationStore, RunnerSettings};
use runner_common::credential_data::CredentialData;
use runner_common::host_context::HostContext;
use runner_sdk::TraceWriter;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Maximum number of session-create retries before giving up.
const MAX_SESSION_CREATE_RETRIES: u32 = 30;

/// Delay between session-create retries (30s in the C# runner).
const SESSION_CREATE_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Long-poll timeout for getting next message (30s).
const GET_MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay before re-creating a session after a conflict (5s).
const SESSION_CONFLICT_DELAY: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Message types (wire format)
// ---------------------------------------------------------------------------

/// A session created on the server side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAgentSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "ownerName")]
    pub owner_name: String,
    #[serde(default, rename = "useFipsEncryption")]
    pub use_fips_encryption: bool,
    #[serde(default, rename = "encryptionKey")]
    pub encryption_key: Option<SessionEncryptionKey>,
}

/// Encryption key data for the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEncryptionKey {
    #[serde(default)]
    pub encrypted: bool,
    #[serde(default)]
    pub value: String,
}

/// A message received from the server via long-poll.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAgentMessage {
    #[serde(rename = "messageId")]
    pub message_id: u64,
    #[serde(rename = "messageType")]
    pub message_type: String,
    #[serde(default)]
    pub body: String,
}

impl TaskAgentMessage {
    /// Returns the message type as a well-known variant.
    pub fn type_kind(&self) -> MessageType {
        match self.message_type.as_str() {
            "PipelineAgentJobRequest" | "AgentJobRequest" => MessageType::JobRequest,
            "JobCancelMessage" => MessageType::JobCancel,
            "AgentRefreshMessage" => MessageType::AgentRefresh,
            "RunnerRefreshMessage" => MessageType::RunnerRefresh,
            "JobMetadataMessage" => MessageType::JobMetadata,
            _ => MessageType::Unknown,
        }
    }
}

/// Well-known message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    JobRequest,
    JobCancel,
    AgentRefresh,
    RunnerRefresh,
    JobMetadata,
    Unknown,
}

// ---------------------------------------------------------------------------
// MessageListener
// ---------------------------------------------------------------------------

/// V1 message listener using the Actions service long-poll API.
///
/// Maps `MessageListener` in the C# runner. This is the legacy path for
/// runners that have not migrated to the V2 broker flow.
pub struct MessageListener {
    context: Arc<HostContext>,
    trace: runner_common::tracing::Tracing,
    session: Option<TaskAgentSession>,
    settings: Option<RunnerSettings>,
    credentials: Option<CredentialData>,
    last_message_id: u64,
    /// Access token for the current session.
    access_token: Option<String>,
    /// Server clock skew detected during authentication.
    clock_skew: Duration,
}

impl MessageListener {
    /// Create a new `MessageListener`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("MessageListener");
        Self {
            context,
            trace,
            session: None,
            settings: None,
            credentials: None,
            last_message_id: 0,
            access_token: None,
            clock_skew: Duration::ZERO,
        }
    }

    /// Create a session on the Actions service.
    ///
    /// Retries up to `MAX_SESSION_CREATE_RETRIES` times on transient failures.
    /// Returns `Err` on permanent failures (e.g. runner removed, auth failure).
    pub async fn create_session_async(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<()> {
        let config_store = ConfigurationStore::new(&self.context);

        let settings = config_store
            .get_settings()
            .context("Failed to load runner settings for session creation")?;

        let credentials = config_store
            .get_credentials()
            .context("Failed to load credentials for session creation")?;

        self.settings = Some(settings.clone());
        self.credentials = Some(credentials.clone());

        self.trace.info(&format!(
            "Attempting to create session for runner '{}' (ID: {})",
            settings.agent_name, settings.agent_id
        ));

        let mut retry_count = 0u32;

        loop {
            if cancel.is_cancelled() {
                return Err(anyhow::anyhow!("Session creation cancelled"));
            }

            match self.try_create_session(&settings, &credentials).await {
                Ok(session) => {
                    self.trace.info(&format!(
                        "Session created: {} (owner: {})",
                        session.session_id, session.owner_name
                    ));
                    self.session = Some(session);
                    return Ok(());
                }
                Err(e) => {
                    // Check for session conflict (HTTP 409)
                    let err_str = format!("{:?}", e);
                    if err_str.contains("409") || err_str.contains("Conflict") {
                        self.trace.warning(&format!(
                            "Session conflict detected. Another runner instance may be running. Retrying in {}s...",
                            SESSION_CONFLICT_DELAY.as_secs()
                        ));
                        tokio::select! {
                            _ = tokio::time::sleep(SESSION_CONFLICT_DELAY) => {},
                            _ = cancel.cancelled() => {
                                return Err(anyhow::anyhow!("Session creation cancelled during conflict delay"));
                            }
                        }
                        continue;
                    }

                    retry_count += 1;
                    if retry_count >= MAX_SESSION_CREATE_RETRIES {
                        return Err(e).context(format!(
                            "Failed to create session after {} retries",
                            MAX_SESSION_CREATE_RETRIES
                        ));
                    }

                    self.trace.warning(&format!(
                        "Failed to create session (attempt {}/{}): {}. Retrying in {}s...",
                        retry_count,
                        MAX_SESSION_CREATE_RETRIES,
                        e,
                        SESSION_CREATE_RETRY_DELAY.as_secs()
                    ));

                    tokio::select! {
                        _ = tokio::time::sleep(SESSION_CREATE_RETRY_DELAY) => {},
                        _ = cancel.cancelled() => {
                            return Err(anyhow::anyhow!("Session creation cancelled during retry delay"));
                        }
                    }
                }
            }
        }
    }

    /// Try to create a session once, returning the session or an error.
    async fn try_create_session(
        &mut self,
        settings: &RunnerSettings,
        credentials: &CredentialData,
    ) -> Result<TaskAgentSession> {
        // Obtain an access token using the credential data
        let token = self.obtain_access_token(credentials).await?;
        self.access_token = Some(token.clone());

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/distributedtask/pools/{}/sessions",
            settings.server_url, settings.pool_id
        );

        let session_request = serde_json::json!({
            "agent": {
                "id": settings.agent_id,
                "name": settings.agent_name,
            },
            "ownerName": format!("runner-{}", settings.agent_name),
        });

        let response = client
            .post(&url)
            .bearer_auth(&token)
            .json(&session_request)
            .send()
            .await
            .context("Failed to send session create request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Session create failed with HTTP {}: {}",
                status.as_u16(),
                body
            ));
        }

        // Detect clock skew from server Date header
        if let Some(date_header) = response.headers().get("date") {
            if let Ok(date_str) = date_header.to_str() {
                if let Ok(server_time) = chrono::DateTime::parse_from_rfc2822(date_str) {
                    let local_time = chrono::Utc::now();
                    let skew = (server_time.timestamp() - local_time.timestamp()).unsigned_abs();
                    self.clock_skew = Duration::from_secs(skew);
                    if skew > 300 {
                        self.trace.warning(&format!(
                            "Significant clock skew detected: {}s between client and server",
                            skew
                        ));
                    }
                }
            }
        }

        let session: TaskAgentSession = response
            .json()
            .await
            .context("Failed to deserialize session response")?;

        Ok(session)
    }

    /// Get the next message from the server via long-poll.
    ///
    /// Returns `None` if no message is available before the timeout expires.
    /// Returns `Some(message)` when a message is received.
    pub async fn get_next_message_async(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<Option<TaskAgentMessage>> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session — call create_session_async first"))?;

        let settings = self
            .settings
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No settings loaded"))?;

        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token available"))?;

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/distributedtask/pools/{}/sessions/{}/messages?lastMessageId={}",
            settings.server_url, settings.pool_id, session.session_id, self.last_message_id
        );

        let response = tokio::select! {
            result = async {
                client
                    .get(&url)
                    .bearer_auth(token)
                    .timeout(GET_MESSAGE_TIMEOUT)
                    .send()
                    .await
            } => result.context("Failed to poll for messages")?,
            _ = cancel.cancelled() => {
                return Ok(None);
            }
        };

        let status = response.status();

        // 200 = message available, 202 = no message (timeout), 401 = refresh auth
        if status == reqwest::StatusCode::ACCEPTED {
            // No message available
            return Ok(None);
        }

        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.trace
                .warning("Got 401 polling messages — refreshing access token");
            if let Some(creds) = &self.credentials {
                match self.obtain_access_token(creds).await {
                    Ok(new_token) => {
                        self.access_token = Some(new_token);
                    }
                    Err(e) => {
                        self.trace.warning(&format!("Failed to refresh access token: {}", e));
                    }
                }
            }
            return Ok(None);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Get message failed with HTTP {}: {}",
                status.as_u16(),
                body
            ));
        }

        let message: TaskAgentMessage = response
            .json()
            .await
            .context("Failed to deserialize message response")?;

        self.last_message_id = message.message_id;

        self.trace.info(&format!(
            "Received message #{}: type={}",
            message.message_id, message.message_type
        ));

        Ok(Some(message))
    }

    /// Delete a message that has been processed.
    pub async fn delete_message_async(&self, message: &TaskAgentMessage) -> Result<()> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?;

        let settings = self
            .settings
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No settings loaded"))?;

        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token available"))?;

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/distributedtask/pools/{}/sessions/{}/messages/{}",
            settings.server_url, settings.pool_id, session.session_id, message.message_id
        );

        let response = client
            .delete(&url)
            .bearer_auth(token)
            .send()
            .await
            .context("Failed to send message delete request")?;

        if !response.status().is_success() {
            self.trace.warning(&format!(
                "Failed to delete message {} (HTTP {})",
                message.message_id,
                response.status().as_u16()
            ));
        }

        Ok(())
    }

    /// Delete the session on the server.
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

        self.trace.info(&format!(
            "Deleting session {}",
            session.session_id
        ));

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let url = format!(
            "{}/_apis/distributedtask/pools/{}/sessions/{}",
            settings.server_url, settings.pool_id, session.session_id
        );

        let _ = client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await;

        Ok(())
    }

    /// Obtain an access token from the credential data.
    async fn obtain_access_token(&self, credentials: &CredentialData) -> Result<String> {
        // If the credential data has an OAuth access token, use that directly
        if let Some(token) = credentials.get_data("accessToken") {
            return Ok(token.clone());
        }

        // If the credential data has a client ID and authorization URL, perform OAuth
        if let (Some(client_id), Some(auth_url)) =
            (&credentials.client_id, &credentials.authorization_url)
        {
            let token = self.exchange_oauth_token(client_id, auth_url).await?;
            return Ok(token);
        }

        // Fallback: check if there is a token in the data map
        if let Some(token) = credentials.get_data("token") {
            return Ok(token.clone());
        }

        Err(anyhow::anyhow!(
            "No access token or credential data available"
        ))
    }

    /// Exchange an OAuth token using client credentials.
    async fn exchange_oauth_token(
        &self,
        _client_id: &str,
        auth_url: &str,
    ) -> Result<String> {
        // Read the RSA key from disk to sign the JWT
        let rsa_key_path = self
            .context
            .get_config_file(runner_common::constants::WellKnownConfigFile::RSACredentials);

        let rsa_pem = std::fs::read_to_string(&rsa_key_path)
            .context("Failed to read RSA key for OAuth token exchange")?;

        let now = chrono::Utc::now();
        let claims = serde_json::json!({
            "sub": _client_id,
            "iss": _client_id,
            "aud": auth_url,
            "nbf": (now - chrono::Duration::minutes(5)).timestamp(),
            "exp": (now + chrono::Duration::minutes(5)).timestamp(),
        });

        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(rsa_pem.as_bytes())
            .context("Failed to parse RSA key")?;

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
            .context("Failed to encode JWT")?;

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

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

        let token_response: TokenResponse = response
            .json()
            .await
            .context("Failed to deserialize OAuth token response")?;

        Ok(token_response.access_token)
    }

    /// Get the current session ID, if any.
    pub fn session_id(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.session_id.as_str())
    }

    /// Get the detected clock skew.
    pub fn clock_skew(&self) -> Duration {
        self.clock_skew
    }
}
