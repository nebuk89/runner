// SelfUpdater mapping `SelfUpdater.cs` (V1 self-update).
// Checks for runner updates, downloads the latest version, and generates
// platform-specific update scripts (bash on Unix, cmd on Windows).

use anyhow::{Context, Result};
use runner_common::constants::{self, WellKnownDirectory};
use runner_common::host_context::HostContext;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Maximum download retry attempts.
const MAX_DOWNLOAD_RETRIES: u32 = constants::RUNNER_DOWNLOAD_RETRY_MAX_ATTEMPTS;

/// Delay between download retries.
const DOWNLOAD_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Agent update message (V1)
// ---------------------------------------------------------------------------

/// The AgentRefreshMessage from the server indicating a new runner version is available.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRefreshMessage {
    #[serde(default, rename = "targetVersion")]
    pub target_version: String,
    #[serde(default, rename = "downloadUrl")]
    pub download_url: Option<String>,
    #[serde(default, rename = "hashValue")]
    pub hash_value: Option<String>,
}

// ---------------------------------------------------------------------------
// SelfUpdater
// ---------------------------------------------------------------------------

/// V1 self-update mechanism.
///
/// Maps `SelfUpdater` in the C# runner. When the server sends an
/// `AgentRefreshMessage`, this module downloads the new runner package
/// and generates a script that replaces the binary on disk.
pub struct SelfUpdater {
    context: Arc<HostContext>,
    trace: Tracing,
}

impl SelfUpdater {
    /// Create a new `SelfUpdater`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("SelfUpdater");
        Self { context, trace }
    }

    /// Check if an update is needed by comparing the target version with the current version.
    pub fn needs_update(&self, target_version: &str) -> bool {
        let current = runner_sdk::build_constants::RunnerPackage::VERSION;

        if target_version.is_empty() {
            self.trace.info("No target version specified — no update needed");
            return false;
        }

        let target_trimmed = target_version.trim().trim_start_matches('v');
        let current_trimmed = current.trim().trim_start_matches('v');

        if target_trimmed == current_trimmed {
            self.trace.info(&format!(
                "Runner is already at version {} — no update needed",
                current
            ));
            return false;
        }

        self.trace.info(&format!(
            "Update available: current={}, target={}",
            current, target_version
        ));
        true
    }

    /// Download the latest runner package and prepare for update.
    ///
    /// Returns the path to the update directory on success.
    pub async fn download_latest_runner(
        &self,
        target_version: &str,
        download_url: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<PathBuf> {
        let update_dir = self.context.get_directory(WellKnownDirectory::Update);

        // Clean the update directory
        if update_dir.exists() {
            std::fs::remove_dir_all(&update_dir)
                .context("Failed to clean update directory")?;
        }
        std::fs::create_dir_all(&update_dir)?;

        // Determine the download URL
        let url = match download_url {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => self.construct_download_url(target_version)?,
        };

        self.trace.info(&format!("Downloading runner from: {}", url));

        // Download with retries
        let archive_path = update_dir.join("runner-update.tar.gz");
        let mut retry_count = 0u32;

        loop {
            if cancel.is_cancelled() {
                return Err(anyhow::anyhow!("Update download cancelled"));
            }

            match self.download_file(&url, &archive_path).await {
                Ok(()) => {
                    self.trace.info("Download completed successfully");
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= MAX_DOWNLOAD_RETRIES {
                        return Err(e).context(format!(
                            "Failed to download runner after {} retries",
                            MAX_DOWNLOAD_RETRIES
                        ));
                    }
                    self.trace.warning(&format!(
                        "Download failed (attempt {}/{}): {}. Retrying...",
                        retry_count, MAX_DOWNLOAD_RETRIES, e
                    ));
                    tokio::select! {
                        _ = tokio::time::sleep(DOWNLOAD_RETRY_DELAY) => {},
                        _ = cancel.cancelled() => {
                            return Err(anyhow::anyhow!("Update download cancelled during retry"));
                        }
                    }
                }
            }
        }

        // Extract the archive
        self.trace.info("Extracting update archive...");
        self.extract_archive(&archive_path, &update_dir)?;

        // Clean up the archive
        let _ = std::fs::remove_file(&archive_path);

        self.trace.info(&format!(
            "Update extracted to {:?}",
            update_dir
        ));

        Ok(update_dir)
    }

    /// Generate the platform-specific update script.
    ///
    /// The update script is responsible for:
    /// 1. Waiting for the current runner process to exit
    /// 2. Copying the new files from the update directory
    /// 3. Restarting the runner
    pub fn generate_update_script(&self, update_dir: &Path) -> Result<PathBuf> {
        let root_dir = self.context.get_directory(WellKnownDirectory::Root);

        #[cfg(unix)]
        {
            self.generate_unix_update_script(&root_dir, update_dir)
        }

        #[cfg(windows)]
        {
            self.generate_windows_update_script(&root_dir, update_dir)
        }
    }

    /// Generate a bash update script for Unix platforms.
    #[cfg(unix)]
    fn generate_unix_update_script(
        &self,
        root_dir: &Path,
        update_dir: &Path,
    ) -> Result<PathBuf> {
        let script_path = root_dir.join("bin").join("RunnerService.sh.update");

        let script = format!(
            r#"#!/bin/bash
# Auto-generated runner update script
set -e

RUNNER_ROOT="{root}"
UPDATE_DIR="{update}"
CURRENT_PID=$$

echo "Runner update starting..."
echo "  Root:   $RUNNER_ROOT"
echo "  Update: $UPDATE_DIR"

# Wait for the current runner process to exit
RUNNER_PID=$(cat "$RUNNER_ROOT/.runner_pid" 2>/dev/null || echo "")
if [ -n "$RUNNER_PID" ] && kill -0 "$RUNNER_PID" 2>/dev/null; then
    echo "Waiting for runner process $RUNNER_PID to exit..."
    for i in $(seq 1 30); do
        if ! kill -0 "$RUNNER_PID" 2>/dev/null; then
            break
        fi
        sleep 1
    done
fi

# Copy updated files
echo "Copying updated files..."
cp -rf "$UPDATE_DIR"/* "$RUNNER_ROOT/"

# Set permissions
chmod +x "$RUNNER_ROOT/bin/Runner.Listener" 2>/dev/null || true
chmod +x "$RUNNER_ROOT/bin/Runner.Worker" 2>/dev/null || true

# Clean up
rm -rf "$UPDATE_DIR"

echo "Update complete. Restarting runner..."
exec "$RUNNER_ROOT/bin/Runner.Listener" run
"#,
            root = root_dir.display(),
            update = update_dir.display(),
        );

        let mut file = std::fs::File::create(&script_path)
            .context("Failed to create update script")?;
        file.write_all(script.as_bytes())?;

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&script_path, perms)?;
        }

        self.trace.info(&format!(
            "Update script generated at {:?}",
            script_path
        ));

        Ok(script_path)
    }

    /// Generate a cmd update script for Windows.
    #[cfg(windows)]
    fn generate_windows_update_script(
        &self,
        root_dir: &Path,
        update_dir: &Path,
    ) -> Result<PathBuf> {
        let script_path = root_dir.join("bin").join("RunnerService.cmd.update");

        let script = format!(
            r#"@echo off
REM Auto-generated runner update script

set RUNNER_ROOT={root}
set UPDATE_DIR={update}

echo Runner update starting...

REM Wait briefly for the runner to exit
timeout /t 5 /nobreak >nul

REM Copy updated files
echo Copying updated files...
xcopy /s /e /y "%UPDATE_DIR%\*" "%RUNNER_ROOT%\"

REM Clean up
rmdir /s /q "%UPDATE_DIR%"

echo Update complete. Restarting runner...
start "" "%RUNNER_ROOT%\bin\Runner.Listener.exe" run
"#,
            root = root_dir.display(),
            update = update_dir.display(),
        );

        std::fs::write(&script_path, &script)
            .context("Failed to create Windows update script")?;

        self.trace.info(&format!(
            "Windows update script generated at {:?}",
            script_path
        ));

        Ok(script_path)
    }

    /// Construct a download URL from the target version.
    fn construct_download_url(&self, target_version: &str) -> Result<String> {
        let version = target_version.trim().trim_start_matches('v');

        let os = match constants::CURRENT_PLATFORM {
            constants::OsPlatform::Linux => "linux",
            constants::OsPlatform::MacOS => "osx",
            constants::OsPlatform::Windows => "win",
        };

        let arch = match constants::CURRENT_ARCHITECTURE {
            constants::Architecture::X64 => "x64",
            constants::Architecture::Arm => "arm",
            constants::Architecture::Arm64 => "arm64",
            constants::Architecture::X86 => "x86",
        };

        let extension = if constants::CURRENT_PLATFORM == constants::OsPlatform::Windows {
            "zip"
        } else {
            "tar.gz"
        };

        Ok(format!(
            "https://github.com/actions/runner/releases/download/v{version}/actions-runner-{os}-{arch}-{version}.{extension}"
        ))
    }

    /// Download a file from a URL to a local path.
    async fn download_file(&self, url: &str, dest: &Path) -> Result<()> {
        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let response = client
            .get(url)
            .send()
            .await
            .context("Failed to send download request")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Download failed with HTTP {}",
                response.status().as_u16()
            ));
        }

        let bytes = response
            .bytes()
            .await
            .context("Failed to read download response body")?;

        std::fs::write(dest, &bytes)
            .context("Failed to write downloaded file to disk")?;

        Ok(())
    }

    /// Extract a tar.gz or zip archive to a destination directory.
    fn extract_archive(&self, archive_path: &Path, dest_dir: &Path) -> Result<()> {
        let extension = archive_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        if archive_path
            .to_string_lossy()
            .ends_with(".tar.gz")
        {
            let file = std::fs::File::open(archive_path)
                .context("Failed to open archive file")?;
            let decoder = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);
            archive
                .unpack(dest_dir)
                .context("Failed to extract tar.gz archive")?;
        } else if extension == "zip" {
            let file = std::fs::File::open(archive_path)
                .context("Failed to open zip archive")?;
            let mut archive = zip::ZipArchive::new(file)
                .context("Failed to read zip archive")?;
            archive
                .extract(dest_dir)
                .context("Failed to extract zip archive")?;
        } else {
            return Err(anyhow::anyhow!(
                "Unknown archive format: {:?}",
                archive_path
            ));
        }

        Ok(())
    }
}
