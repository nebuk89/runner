// SelfUpdaterV2 mapping `SelfUpdaterV2.cs` (V2 self-update).
// The V2 update mechanism receives the download URL and SHA256 hash
// directly in the RunnerRefreshMessage, eliminating the need to
// construct URLs or verify against a separate source.

use anyhow::{Context, Result};
use runner_common::constants::{self, WellKnownDirectory};
use runner_common::host_context::HostContext;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Maximum download retry attempts.
const MAX_DOWNLOAD_RETRIES: u32 = constants::RUNNER_DOWNLOAD_RETRY_MAX_ATTEMPTS;

/// Delay between download retries.
const DOWNLOAD_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

// ---------------------------------------------------------------------------
// V2 update message
// ---------------------------------------------------------------------------

/// The RunnerRefreshMessage from the broker indicating a new runner version is available.
/// V2 messages include the full download URL and expected SHA256 hash.
#[derive(Debug, Clone, Deserialize)]
pub struct RunnerRefreshMessage {
    #[serde(default, rename = "targetVersion")]
    pub target_version: String,
    #[serde(default, rename = "downloadUrl")]
    pub download_url: String,
    #[serde(default, rename = "hashValue")]
    pub hash_value: String,
}

// ---------------------------------------------------------------------------
// SelfUpdaterV2
// ---------------------------------------------------------------------------

/// V2 self-update mechanism.
///
/// Maps `SelfUpdaterV2` in the C# runner. Similar to V1 but the download
/// URL and SHA256 hash come directly from the server message, and the
/// hash is verified before applying the update.
pub struct SelfUpdaterV2 {
    context: Arc<HostContext>,
    trace: Tracing,
}

impl SelfUpdaterV2 {
    /// Create a new `SelfUpdaterV2`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("SelfUpdaterV2");
        Self { context, trace }
    }

    /// Check if an update is needed by comparing versions.
    pub fn needs_update(&self, target_version: &str) -> bool {
        let current = runner_sdk::build_constants::RunnerPackage::VERSION;

        if target_version.is_empty() {
            return false;
        }

        let target_trimmed = target_version.trim().trim_start_matches('v');
        let current_trimmed = current.trim().trim_start_matches('v');

        if target_trimmed == current_trimmed {
            self.trace.info(&format!(
                "Runner is already at version {} — no update needed (V2)",
                current
            ));
            return false;
        }

        self.trace.info(&format!(
            "V2 update available: current={}, target={}",
            current, target_version
        ));
        true
    }

    /// Download and verify the runner update package.
    ///
    /// The `message` must contain a valid `download_url` and `hash_value`.
    /// Returns the path to the update directory on success.
    pub async fn download_and_verify(
        &self,
        message: &RunnerRefreshMessage,
        cancel: CancellationToken,
    ) -> Result<PathBuf> {
        if message.download_url.is_empty() {
            return Err(anyhow::anyhow!(
                "V2 update message has no download URL"
            ));
        }

        let update_dir = self.context.get_directory(WellKnownDirectory::Update);

        // Clean the update directory
        if update_dir.exists() {
            std::fs::remove_dir_all(&update_dir)
                .context("Failed to clean update directory for V2 update")?;
        }
        std::fs::create_dir_all(&update_dir)?;

        let archive_name = if constants::CURRENT_PLATFORM == constants::OsPlatform::Windows {
            "runner-update.zip"
        } else {
            "runner-update.tar.gz"
        };
        let archive_path = update_dir.join(archive_name);

        // Download with retries
        self.trace.info(&format!(
            "V2: Downloading runner from: {}",
            message.download_url
        ));

        let mut retry_count = 0u32;

        loop {
            if cancel.is_cancelled() {
                return Err(anyhow::anyhow!("V2 update download cancelled"));
            }

            match self.download_file(&message.download_url, &archive_path).await {
                Ok(()) => break,
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= MAX_DOWNLOAD_RETRIES {
                        return Err(e).context(format!(
                            "V2: Failed to download runner after {} retries",
                            MAX_DOWNLOAD_RETRIES
                        ));
                    }
                    self.trace.warning(&format!(
                        "V2: Download failed (attempt {}/{}): {}",
                        retry_count, MAX_DOWNLOAD_RETRIES, e
                    ));
                    tokio::select! {
                        _ = tokio::time::sleep(DOWNLOAD_RETRY_DELAY) => {},
                        _ = cancel.cancelled() => {
                            return Err(anyhow::anyhow!("V2 update download cancelled during retry"));
                        }
                    }
                }
            }
        }

        // Verify SHA256 hash
        if !message.hash_value.is_empty() {
            self.trace.info("V2: Verifying SHA256 hash...");
            self.verify_hash(&archive_path, &message.hash_value)?;
            self.trace.info("V2: SHA256 hash verified successfully");
        } else {
            self.trace
                .warning("V2: No hash value provided — skipping verification");
        }

        // Extract the archive
        self.trace.info("V2: Extracting update archive...");
        self.extract_archive(&archive_path, &update_dir)?;

        // Remove the archive
        let _ = std::fs::remove_file(&archive_path);

        self.trace.info(&format!(
            "V2: Update extracted to {:?}",
            update_dir
        ));

        Ok(update_dir)
    }

    /// Verify the SHA256 hash of the downloaded file.
    fn verify_hash(&self, file_path: &Path, expected_hex: &str) -> Result<()> {
        let data = std::fs::read(file_path)
            .context("Failed to read file for hash verification")?;

        let mut hasher = Sha256::new();
        hasher.update(&data);
        let computed = hasher.finalize();
        let computed_hex = hex::encode(computed);

        let expected_lower = expected_hex.to_lowercase();
        if computed_hex != expected_lower {
            return Err(anyhow::anyhow!(
                "SHA256 mismatch: expected={}, computed={}",
                expected_lower,
                computed_hex
            ));
        }

        Ok(())
    }

    /// Generate the platform-specific update script (delegates to the V1 updater logic).
    pub fn generate_update_script(&self, update_dir: &Path) -> Result<PathBuf> {
        // Reuse the V1 updater's script generation
        let v1 = super::self_updater::SelfUpdater::new(self.context.clone());
        v1.generate_update_script(update_dir)
    }

    /// Download a file from a URL to a local path.
    async fn download_file(&self, url: &str, dest: &Path) -> Result<()> {
        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let response = client
            .get(url)
            .send()
            .await
            .context("V2: Failed to send download request")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "V2: Download failed with HTTP {}",
                response.status().as_u16()
            ));
        }

        let bytes = response
            .bytes()
            .await
            .context("V2: Failed to read download body")?;

        std::fs::write(dest, &bytes)
            .context("V2: Failed to write downloaded file")?;

        Ok(())
    }

    /// Extract a tar.gz or zip archive to a destination directory.
    fn extract_archive(&self, archive_path: &Path, dest_dir: &Path) -> Result<()> {
        if archive_path.to_string_lossy().ends_with(".tar.gz") {
            let file = std::fs::File::open(archive_path)?;
            let decoder = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);
            archive.unpack(dest_dir)?;
        } else if archive_path
            .extension()
            .map_or(false, |e| e == "zip")
        {
            let file = std::fs::File::open(archive_path)?;
            let mut archive = zip::ZipArchive::new(file)?;
            archive.extract(dest_dir)?;
        } else {
            return Err(anyhow::anyhow!(
                "V2: Unknown archive format: {:?}",
                archive_path
            ));
        }

        Ok(())
    }
}
