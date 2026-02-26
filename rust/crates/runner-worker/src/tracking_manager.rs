// TrackingManager mapping `TrackingManager.cs`.
// Manages pipeline directory tracking — allocates build directories,
// manages the tracking config so that workspaces persist across runs.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use runner_common::host_context::HostContext;

use crate::worker::AgentJobRequestMessage;

/// Tracking configuration persisted between runs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackingConfig {
    /// The pipeline directory identifier.
    pub pipeline_directory: String,

    /// The workspace directory.
    pub workspace_directory: String,

    /// The repository being tracked.
    pub repository_name: String,

    /// Build directories already allocated.
    pub build_directories: HashMap<String, String>,

    /// Last run timestamp.
    pub last_run_on: String,
}

/// Manages pipeline directory tracking and workspace allocation.
pub struct TrackingManager {
    /// Root work directory.
    work_directory: String,

    /// Path to the tracking config file.
    tracking_config_path: PathBuf,
}

impl TrackingManager {
    /// Create a new `TrackingManager`.
    pub fn new(host_context: &HostContext) -> Self {
        let work_directory = std::env::var("RUNNER_WORK_DIRECTORY")
            .unwrap_or_else(|_| {
                host_context
                    .get_directory(runner_common::constants::WellKnownDirectory::Work)
                    .to_string_lossy()
                    .to_string()
            });

        let tracking_config_path = PathBuf::from(&work_directory).join(".tracking_config.json");

        Self {
            work_directory,
            tracking_config_path,
        }
    }

    /// Prepare the pipeline directory for a job.
    ///
    /// Returns `(pipeline_dir, workspace_dir, temp_dir)`.
    ///
    /// Reads existing tracking config or creates a new one.
    /// Allocates a unique numbered directory under the work root.
    pub fn prepare_pipeline_directory(
        &self,
        message: &AgentJobRequestMessage,
    ) -> Result<(String, String, String)> {
        let repo_name = self.extract_repository_name(message);

        // Try to load existing tracking config
        let tracking = self.load_or_create_tracking(&repo_name)?;

        let pipeline_dir = PathBuf::from(&self.work_directory).join(&tracking.pipeline_directory);
        let workspace_dir = pipeline_dir.join(&tracking.workspace_directory);
        let temp_dir = pipeline_dir.join("_temp");

        // Create all directories
        std::fs::create_dir_all(&pipeline_dir)
            .with_context(|| format!("Failed to create pipeline directory: {:?}", pipeline_dir))?;
        std::fs::create_dir_all(&workspace_dir)
            .with_context(|| format!("Failed to create workspace directory: {:?}", workspace_dir))?;
        std::fs::create_dir_all(&temp_dir)
            .with_context(|| format!("Failed to create temp directory: {:?}", temp_dir))?;

        // Save tracking config
        self.save_tracking(&tracking)?;

        Ok((
            pipeline_dir.to_string_lossy().to_string(),
            workspace_dir.to_string_lossy().to_string(),
            temp_dir.to_string_lossy().to_string(),
        ))
    }

    /// Extract the repository name from the job message.
    fn extract_repository_name(&self, message: &AgentJobRequestMessage) -> String {
        // Look for the repository name in variables
        for (var_name, var_value) in &message.variables {
            if var_name.eq_ignore_ascii_case("system.github.repository") {
                return var_value.value.clone();
            }
        }
        message.job_display_name.clone()
    }

    /// Load existing tracking config or create a new one.
    fn load_or_create_tracking(&self, repo_name: &str) -> Result<TrackingConfig> {
        // Try to load existing configs
        if self.tracking_config_path.exists() {
            let content = std::fs::read_to_string(&self.tracking_config_path)?;
            if let Ok(mut config) = serde_json::from_str::<TrackingConfig>(&content) {
                // Check if this is for the same repo
                if config.repository_name == repo_name {
                    config.last_run_on = chrono::Utc::now().to_rfc3339();
                    return Ok(config);
                }
            }
        }

        // Also check numbered directories for existing tracking files
        let work_path = Path::new(&self.work_directory);
        if work_path.exists() {
            for entry in std::fs::read_dir(work_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    let tracking_file = path.join(".tracking");
                    if tracking_file.exists() {
                        if let Ok(content) = std::fs::read_to_string(&tracking_file) {
                            if let Ok(config) =
                                serde_json::from_str::<TrackingConfig>(&content)
                            {
                                if config.repository_name == repo_name {
                                    let mut config = config;
                                    config.last_run_on = chrono::Utc::now().to_rfc3339();
                                    return Ok(config);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Create new tracking config
        let pipeline_dir = self.allocate_directory()?;
        let workspace_name = self.sanitize_directory_name(repo_name);

        Ok(TrackingConfig {
            pipeline_directory: pipeline_dir,
            workspace_directory: workspace_name,
            repository_name: repo_name.to_string(),
            build_directories: HashMap::new(),
            last_run_on: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Allocate a new numbered directory.
    fn allocate_directory(&self) -> Result<String> {
        let work_path = Path::new(&self.work_directory);
        std::fs::create_dir_all(work_path)?;

        // Find the next available number
        let mut max_num: u32 = 0;
        if let Ok(entries) = std::fs::read_dir(work_path) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(num) = name.parse::<u32>() {
                        max_num = max_num.max(num);
                    }
                }
            }
        }

        let dir_name = (max_num + 1).to_string();
        Ok(dir_name)
    }

    /// Sanitize a repository name for use as a directory name.
    fn sanitize_directory_name(&self, name: &str) -> String {
        // "owner/repo" → "repo"
        let short_name = name.rsplit('/').next().unwrap_or(name);

        // Replace invalid chars with underscores
        short_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// Save the tracking config to disk.
    fn save_tracking(&self, config: &TrackingConfig) -> Result<()> {
        // Save to the global tracking config
        let content = serde_json::to_string_pretty(config)?;

        if let Some(parent) = self.tracking_config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&self.tracking_config_path, &content)?;

        // Also save in the pipeline directory itself
        let pipeline_tracking = PathBuf::from(&self.work_directory)
            .join(&config.pipeline_directory)
            .join(".tracking");

        if let Some(parent) = pipeline_tracking.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&pipeline_tracking, &content)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_directory_name() {
        let mgr = TrackingManager {
            work_directory: "/tmp/work".to_string(),
            tracking_config_path: PathBuf::from("/tmp/work/.tracking_config.json"),
        };

        assert_eq!(mgr.sanitize_directory_name("owner/repo"), "repo");
        assert_eq!(mgr.sanitize_directory_name("my-repo"), "my-repo");
        assert_eq!(mgr.sanitize_directory_name("bad name!"), "bad_name_");
    }

    #[test]
    fn test_allocate_directory_empty() {
        let temp = tempfile::tempdir().unwrap();
        let mgr = TrackingManager {
            work_directory: temp.path().to_string_lossy().to_string(),
            tracking_config_path: temp.path().join(".tracking_config.json"),
        };

        let dir = mgr.allocate_directory().unwrap();
        assert_eq!(dir, "1");
    }

    #[test]
    fn test_allocate_directory_with_existing() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("1")).unwrap();
        std::fs::create_dir(temp.path().join("2")).unwrap();
        std::fs::create_dir(temp.path().join("5")).unwrap();

        let mgr = TrackingManager {
            work_directory: temp.path().to_string_lossy().to_string(),
            tracking_config_path: temp.path().join(".tracking_config.json"),
        };

        let dir = mgr.allocate_directory().unwrap();
        assert_eq!(dir, "6");
    }
}
