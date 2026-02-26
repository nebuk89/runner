// RunnerConfigUpdater mapping `RunnerConfigUpdater.cs`.
// Handles the RunnerRefreshConfig message from the server, which instructs
// the runner to refresh its configuration (e.g. labels, runner group).

use anyhow::{Context, Result};
use runner_common::config_store::ConfigurationStore;
use runner_common::host_context::HostContext;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use serde::Deserialize;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Config refresh message
// ---------------------------------------------------------------------------

/// A message from the server instructing the runner to refresh its configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct RunnerRefreshConfigMessage {
    #[serde(default, rename = "runnerId")]
    pub runner_id: u64,
    #[serde(default, rename = "labels")]
    pub labels: Option<Vec<String>>,
    #[serde(default, rename = "runnerGroup")]
    pub runner_group: Option<String>,
    #[serde(default, rename = "runnerGroupId")]
    pub runner_group_id: Option<i32>,
}

// ---------------------------------------------------------------------------
// RunnerConfigUpdater
// ---------------------------------------------------------------------------

/// Processes runner configuration refresh messages.
///
/// When the server sends a `RunnerRefreshConfig` message, the runner
/// updates its local settings to match the server-side configuration.
pub struct RunnerConfigUpdater {
    context: Arc<HostContext>,
    trace: Tracing,
}

impl RunnerConfigUpdater {
    /// Create a new `RunnerConfigUpdater`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("RunnerConfigUpdater");
        Self { context, trace }
    }

    /// Process a configuration refresh message.
    ///
    /// Updates the local runner settings with the values from the message.
    /// Returns `true` if the configuration was updated and the runner should
    /// restart to pick up the changes.
    pub fn process_config_refresh(
        &self,
        message: &RunnerRefreshConfigMessage,
    ) -> Result<bool> {
        self.trace.info(&format!(
            "Processing config refresh for runner ID {}",
            message.runner_id
        ));

        let config_store = ConfigurationStore::new(&self.context);

        if !config_store.is_configured() {
            self.trace
                .warning("Runner is not configured — ignoring config refresh");
            return Ok(false);
        }

        let mut settings = config_store
            .get_settings()
            .context("Failed to load settings for config refresh")?;

        let mut updated = false;

        // Update runner group if specified
        if let Some(ref group_name) = message.runner_group {
            if settings.pool_name != *group_name {
                self.trace.info(&format!(
                    "Updating runner group: '{}' -> '{}'",
                    settings.pool_name, group_name
                ));
                settings.pool_name = group_name.clone();
                updated = true;
            }
        }

        if let Some(group_id) = message.runner_group_id {
            if settings.pool_id != group_id {
                self.trace.info(&format!(
                    "Updating runner group ID: {} -> {}",
                    settings.pool_id, group_id
                ));
                settings.pool_id = group_id;
                updated = true;
            }
        }

        if updated {
            config_store
                .save_settings(&settings)
                .context("Failed to save updated settings")?;
            self.trace.info("Runner configuration updated successfully");
        } else {
            self.trace
                .info("No configuration changes detected — nothing to update");
        }

        Ok(updated)
    }
}
