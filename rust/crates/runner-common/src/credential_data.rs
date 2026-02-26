// CredentialData mapping `CredentialData.cs`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Stored credential information used by the runner to authenticate
/// with the GitHub Actions service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialData {
    /// The authentication scheme name (e.g. "OAuth", "OAuthAccessToken").
    #[serde(default, rename = "Scheme")]
    pub scheme: String,

    /// Optional authorization URL for the credential provider.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "AuthorizationUrl")]
    pub authorization_url: Option<String>,

    /// Optional client ID for the credential provider.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ClientId")]
    pub client_id: Option<String>,

    /// Arbitrary key/value data associated with the credential.
    #[serde(default, rename = "Data")]
    pub data: HashMap<String, String>,
}

impl CredentialData {
    /// Create a new empty `CredentialData` with the given scheme.
    pub fn new(scheme: &str) -> Self {
        Self {
            scheme: scheme.to_string(),
            authorization_url: None,
            client_id: None,
            data: HashMap::new(),
        }
    }

    /// Get a data value by key (case-insensitive lookup).
    pub fn get_data(&self, key: &str) -> Option<&String> {
        // Perform case-insensitive lookup to mirror C# StringComparer.OrdinalIgnoreCase
        self.data
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }
}
