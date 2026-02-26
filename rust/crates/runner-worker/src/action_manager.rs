// ActionManager mapping `ActionManager.cs`.
// Downloads actions from GitHub, caches resolved actions, and handles container images.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use runner_common::constants;
use runner_common::host_context::HostContext;

use crate::execution_context::ExecutionContext;
use crate::worker::{ActionReference, JobStep};

/// Result of preparing actions.
#[derive(Debug)]
pub struct PrepareResult {
    /// Map of action reference key → resolved local directory.
    pub resolved_actions: HashMap<String, String>,

    /// Any warnings generated during resolution.
    pub warnings: Vec<String>,
}

/// Manages action download, caching, and resolution.
pub struct ActionManager {
    /// Cache of already-resolved actions (key → local path).
    cache: HashMap<String, String>,
}

impl ActionManager {
    /// Create a new `ActionManager`.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Prepare all actions referenced by steps.
    ///
    /// Downloads action repositories from GitHub, extracts them, and resolves
    /// their local paths in the runner's actions directory.
    pub async fn prepare_actions_async(
        &mut self,
        context: &mut ExecutionContext,
        steps: &[JobStep],
    ) -> Result<PrepareResult> {
        let mut result = PrepareResult {
            resolved_actions: HashMap::new(),
            warnings: Vec::new(),
        };

        let actions_dir = context
            .host_context()
            .get_directory(constants::WellKnownDirectory::Actions);

        for step in steps {
            if let Some(action_ref) = step.action_reference() {
                let cache_key = self.build_cache_key(&action_ref);

                // Check if already resolved
                if let Some(cached_path) = self.cache.get(&cache_key) {
                    result
                        .resolved_actions
                        .insert(cache_key.clone(), cached_path.clone());
                    continue;
                }

                // Resolve the action
                match self
                    .resolve_action(context, &action_ref, &actions_dir)
                    .await
                {
                    Ok(resolved_path) => {
                        context.debug(&format!(
                            "Resolved action '{}' to '{}'",
                            cache_key, resolved_path
                        ));
                        self.cache
                            .insert(cache_key.clone(), resolved_path.clone());
                        result
                            .resolved_actions
                            .insert(cache_key, resolved_path);
                    }
                    Err(e) => {
                        let warning = format!(
                            "Failed to resolve action '{}': {:#}",
                            cache_key, e
                        );
                        context.warning(&warning);
                        result.warnings.push(warning);
                    }
                }
            }
        }

        Ok(result)
    }

    /// Build a cache key for an action reference.
    fn build_cache_key(&self, action_ref: &ActionReference) -> String {
        if action_ref.path.is_empty() {
            format!("{}@{}", action_ref.name, action_ref.git_ref)
        } else {
            format!("{}/{}@{}", action_ref.name, action_ref.path, action_ref.git_ref)
        }
    }

    /// Resolve a single action reference to a local directory.
    async fn resolve_action(
        &self,
        context: &mut ExecutionContext,
        action_ref: &ActionReference,
        actions_dir: &Path,
    ) -> Result<String> {
        match action_ref.repository_type.as_str() {
            "GitHub" | "" => self.resolve_github_action(context, action_ref, actions_dir).await,
            "Container" => self.resolve_container_action(context, action_ref),
            other => {
                anyhow::bail!("Unsupported repository type: {}", other);
            }
        }
    }

    /// Resolve a GitHub-hosted action.
    async fn resolve_github_action(
        &self,
        context: &mut ExecutionContext,
        action_ref: &ActionReference,
        actions_dir: &Path,
    ) -> Result<String> {
        // Build the target directory
        let parts: Vec<&str> = action_ref.name.splitn(2, '/').collect();
        if parts.len() < 2 {
            anyhow::bail!("Invalid action reference: {}", action_ref.name);
        }

        let owner = parts[0];
        let repo = parts[1];
        let git_ref = &action_ref.git_ref;

        let action_dir = actions_dir
            .join(owner)
            .join(repo)
            .join(git_ref);

        // Check if already cached on disk
        if action_dir.exists() {
            let sub_path = if action_ref.path.is_empty() {
                action_dir.clone()
            } else {
                action_dir.join(&action_ref.path)
            };
            return Ok(sub_path.to_string_lossy().to_string());
        }

        // Check for action archive cache
        let archive_cache = std::env::var(constants::variables::agent::ACTION_ARCHIVE_CACHE_DIRECTORY).ok();
        if let Some(ref cache_dir) = archive_cache {
            let cache_path = PathBuf::from(cache_dir)
                .join(format!("{}_{}_{}.tar.gz", owner, repo, git_ref));

            if cache_path.exists() {
                context.info(&format!("Using cached action archive: {:?}", cache_path));
                self.extract_archive(&cache_path, &action_dir)?;

                let sub_path = if action_ref.path.is_empty() {
                    action_dir.clone()
                } else {
                    action_dir.join(&action_ref.path)
                };
                return Ok(sub_path.to_string_lossy().to_string());
            }
        }

        // Download from GitHub
        context.info(&format!(
            "Downloading action '{}/{}@{}'...",
            owner, repo, git_ref
        ));

        let download_url = format!(
            "https://api.github.com/repos/{}/{}/tarball/{}",
            owner, repo, git_ref
        );

        // Find the access token from endpoints
        let token = self.find_access_token(context);

        let temp_dir = context.global().temp_directory.clone();
        let archive_path = PathBuf::from(&temp_dir)
            .join(format!("action_{}.tar.gz", uuid::Uuid::new_v4().as_simple()));

        self.download_archive(&download_url, &archive_path, token.as_deref())
            .await
            .context("Failed to download action archive")?;

        // Extract
        self.extract_archive(&archive_path, &action_dir)?;

        // Clean up temp archive
        let _ = std::fs::remove_file(&archive_path);

        let sub_path = if action_ref.path.is_empty() {
            action_dir.clone()
        } else {
            action_dir.join(&action_ref.path)
        };

        Ok(sub_path.to_string_lossy().to_string())
    }

    /// Resolve a container action (just returns the image reference).
    fn resolve_container_action(
        &self,
        context: &mut ExecutionContext,
        action_ref: &ActionReference,
    ) -> Result<String> {
        context.debug(&format!("Container action: {}", action_ref.name));
        Ok(action_ref.name.clone())
    }

    /// Download an archive file from a URL.
    async fn download_archive(
        &self,
        url: &str,
        destination: &Path,
        token: Option<&str>,
    ) -> Result<()> {
        let client = reqwest::Client::new();
        let mut request = client.get(url);

        if let Some(token) = token {
            request = request.header("Authorization", format!("token {}", token));
        }

        request = request.header("User-Agent", "GitHubActionsRunner");

        let response = request
            .send()
            .await
            .context("HTTP request failed")?
            .error_for_status()
            .context("HTTP response error")?;

        let bytes = response.bytes().await.context("Failed to read response body")?;

        // Ensure parent directory exists
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(destination, &bytes)
            .with_context(|| format!("Failed to write archive to {:?}", destination))?;

        Ok(())
    }

    /// Extract a tar.gz archive to a destination directory.
    fn extract_archive(&self, archive: &Path, destination: &Path) -> Result<()> {
        use flate2::read::GzDecoder;
        use tar::Archive;

        let file = std::fs::File::open(archive)
            .with_context(|| format!("Failed to open archive {:?}", archive))?;

        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);

        // Create destination
        std::fs::create_dir_all(destination)?;

        // Extract, stripping the top-level directory (GitHub tarballs have one)
        for entry_result in archive.entries()? {
            let mut entry = entry_result?;
            let path = entry.path()?.into_owned();

            // Strip the first component (e.g. "owner-repo-sha/")
            let components: Vec<_> = path.components().collect();
            if components.len() <= 1 {
                continue;
            }

            let stripped: PathBuf = components[1..].iter().collect();
            let target = destination.join(&stripped);

            // Ensure parent directory exists
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }

            entry.unpack(&target)?;
        }

        Ok(())
    }

    /// Find the access token from the execution context's endpoints.
    fn find_access_token(&self, context: &ExecutionContext) -> Option<String> {
        let global = context.global();
        for endpoint in &global.endpoints {
            if endpoint.name == "SystemVssConnection" {
                if let Some(ref auth) = endpoint.authorization {
                    if let Some(token) = auth.parameters.get("AccessToken") {
                        return Some(token.clone());
                    }
                }
            }
        }
        None
    }
}

impl Default for ActionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cache_key() {
        let mgr = ActionManager::new();
        let action_ref = ActionReference {
            name: "actions/checkout".to_string(),
            git_ref: "v4".to_string(),
            path: String::new(),
            repository_type: "GitHub".to_string(),
            ref_type: String::new(),
            extra: Default::default(),
        };
        assert_eq!(mgr.build_cache_key(&action_ref), "actions/checkout@v4");
    }

    #[test]
    fn test_build_cache_key_with_path() {
        let mgr = ActionManager::new();
        let action_ref = ActionReference {
            name: "actions/runner".to_string(),
            git_ref: "main".to_string(),
            path: "sub/action".to_string(),
            repository_type: "GitHub".to_string(),
            ref_type: String::new(),
            extra: Default::default(),
        };
        assert_eq!(
            mgr.build_cache_key(&action_ref),
            "actions/runner/sub/action@main"
        );
    }
}
