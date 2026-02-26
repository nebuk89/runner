// RsaKeyManager mapping `RSAKeyManager.cs`.
// Generates RSA key pairs and saves them to disk for OAuth credential exchange.

use anyhow::{Context, Result};
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::RsaPrivateKey;
use runner_common::constants::WellKnownConfigFile;
use runner_common::host_context::HostContext;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use std::sync::Arc;

/// RSA key size in bits.
const RSA_KEY_SIZE: usize = 2048;

/// Manages RSA key pairs for OAuth credential exchange.
///
/// Maps `RSAKeyManager` in the C# runner. The RSA key pair is used to
/// sign JWTs that are exchanged for OAuth access tokens.
pub struct RsaKeyManager {
    context: Arc<HostContext>,
    trace: Tracing,
}

impl RsaKeyManager {
    /// Create a new `RsaKeyManager`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("RsaKeyManager");
        Self { context, trace }
    }

    /// Generate a new RSA key pair and save the private key to disk.
    ///
    /// Returns the public key in PEM format (for sending to the server).
    pub fn generate_and_save_key(&self) -> Result<String> {
        self.trace.info("Generating RSA key pair...");

        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, RSA_KEY_SIZE)
            .context("Failed to generate RSA private key")?;

        // Serialize private key to PEM
        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .context("Failed to serialize RSA private key to PEM")?;

        // Serialize public key to PEM
        let public_key = private_key.to_public_key();
        let public_pem = public_key
            .to_public_key_pem(LineEnding::LF)
            .context("Failed to serialize RSA public key to PEM")?;

        // Save the private key to disk
        let key_path = self
            .context
            .get_config_file(WellKnownConfigFile::RSACredentials);

        // Delete existing key file
        if key_path.exists() {
            std::fs::remove_file(&key_path)
                .context("Failed to delete existing RSA key file")?;
        }

        std::fs::write(&key_path, private_pem.as_bytes())
            .context("Failed to write RSA private key to disk")?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&key_path, perms)
                .context("Failed to set permissions on RSA key file")?;
        }

        self.trace.info(&format!(
            "RSA key pair generated and saved to {:?}",
            key_path
        ));

        Ok(public_pem)
    }

    /// Load the existing RSA private key from disk.
    pub fn load_private_key(&self) -> Result<String> {
        let key_path = self
            .context
            .get_config_file(WellKnownConfigFile::RSACredentials);

        let pem = std::fs::read_to_string(&key_path)
            .context("Failed to read RSA private key from disk")?;

        Ok(pem)
    }

    /// Check whether an RSA key exists on disk.
    pub fn has_key(&self) -> bool {
        let key_path = self
            .context
            .get_config_file(WellKnownConfigFile::RSACredentials);
        key_path.exists()
    }

    /// Delete the RSA key from disk.
    pub fn delete_key(&self) -> Result<()> {
        let key_path = self
            .context
            .get_config_file(WellKnownConfigFile::RSACredentials);

        if key_path.exists() {
            std::fs::remove_file(&key_path)
                .context("Failed to delete RSA key file")?;
            self.trace.info("RSA key deleted");
        }

        Ok(())
    }
}
