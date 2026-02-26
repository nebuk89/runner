// GitHubContext mapping `GitHubContext.cs`.
// Populates the `github.*` expression context from the job message and environment.

use std::collections::HashMap;

use crate::worker::AgentJobRequestMessage;

/// The `github` context available in expressions.
///
/// Populated from the job message and environment variables.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GitHubContext {
    /// The workflow name.
    pub workflow: String,

    /// The workflow ref (SHA).
    pub workflow_ref: String,

    /// The workflow SHA.
    pub workflow_sha: String,

    /// The run ID.
    pub run_id: String,

    /// The run number.
    pub run_number: String,

    /// The run attempt.
    pub run_attempt: String,

    /// The actor (user who triggered the workflow).
    pub actor: String,

    /// The triggering actor.
    pub triggering_actor: String,

    /// The repository (owner/name).
    pub repository: String,

    /// The repository owner.
    pub repository_owner: String,

    /// Repository ID.
    pub repository_id: String,

    /// Repository owner ID.
    pub repository_owner_id: String,

    /// The event name (push, pull_request, etc.).
    pub event_name: String,

    /// The event payload as a JSON value.
    pub event: serde_json::Value,

    /// The SHA that triggered the workflow.
    pub sha: String,

    /// The ref that triggered the workflow.
    #[serde(rename = "ref")]
    pub git_ref: String,

    /// The head ref (for PRs).
    pub head_ref: String,

    /// The base ref (for PRs).
    pub base_ref: String,

    /// The server URL (e.g., https://github.com).
    pub server_url: String,

    /// The API URL.
    pub api_url: String,

    /// The GraphQL URL.
    pub graphql_url: String,

    /// The ref name (branch or tag name without refs/heads/ or refs/tags/).
    pub ref_name: String,

    /// Whether the ref is protected.
    pub ref_protected: bool,

    /// The ref type (branch or tag).
    pub ref_type: String,

    /// The workspace path.
    pub workspace: String,

    /// The job name.
    pub job: String,

    /// The action name (current step reference).
    pub action: String,

    /// The action path.
    pub action_path: String,

    /// The action ref.
    pub action_ref: String,

    /// The action repository.
    pub action_repository: String,

    /// The action status.
    pub action_status: String,

    /// The token.
    pub token: String,

    /// Retention days.
    pub retention_days: String,

    /// The repository URL.
    pub repositoryurl: String,

    /// Extra fields from the job message.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl GitHubContext {
    /// Build the GitHubContext from a job message and known variables.
    pub fn from_message(
        message: &AgentJobRequestMessage,
        variables: &HashMap<String, String>,
    ) -> Self {
        let get_var = |name: &str| -> String {
            variables
                .get(name)
                .cloned()
                .unwrap_or_default()
        };

        let repository = get_var("system.github.repository");
        let repo_parts: Vec<&str> = repository.splitn(2, '/').collect();
        let repository_owner = if repo_parts.len() >= 2 {
            repo_parts[0].to_string()
        } else {
            String::new()
        };

        let git_ref = get_var("system.github.ref");
        let ref_name = Self::extract_ref_name(&git_ref);
        let ref_type = Self::extract_ref_type(&git_ref);

        // Parse event payload
        let event_str = get_var("system.github.event");
        let event: serde_json::Value = serde_json::from_str(&event_str)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let server_url = get_var("system.github.server_url");
        let api_url = if server_url == "https://github.com" || server_url.is_empty() {
            "https://api.github.com".to_string()
        } else {
            format!("{}/api/v3", server_url)
        };
        let graphql_url = if server_url == "https://github.com" || server_url.is_empty() {
            "https://api.github.com/graphql".to_string()
        } else {
            format!("{}/api/graphql", server_url)
        };

        Self {
            workflow: get_var("system.github.workflow"),
            workflow_ref: get_var("system.github.workflow_ref"),
            workflow_sha: get_var("system.github.workflow_sha"),
            run_id: get_var("system.github.run_id"),
            run_number: get_var("system.github.run_number"),
            run_attempt: get_var("system.github.run_attempt"),
            actor: get_var("system.github.actor"),
            triggering_actor: get_var("system.github.triggering_actor"),
            repository: repository.clone(),
            repository_owner,
            repository_id: get_var("system.github.repository_id"),
            repository_owner_id: get_var("system.github.repository_owner_id"),
            event_name: get_var("system.github.event_name"),
            event,
            sha: get_var("system.github.sha"),
            git_ref,
            head_ref: get_var("system.github.head_ref"),
            base_ref: get_var("system.github.base_ref"),
            server_url: if server_url.is_empty() {
                "https://github.com".to_string()
            } else {
                server_url
            },
            api_url,
            graphql_url,
            ref_name,
            ref_protected: get_var("system.github.ref_protected")
                .eq_ignore_ascii_case("true"),
            ref_type,
            workspace: get_var("system.github.workspace"),
            job: message.job_display_name.clone(),
            action: String::new(),
            action_path: String::new(),
            action_ref: String::new(),
            action_repository: String::new(),
            action_status: String::new(),
            token: get_var("system.github.token"),
            retention_days: get_var("system.github.retention_days"),
            repositoryurl: format!(
                "{}/{}",
                "https://github.com",
                repository
            ),
            extra: HashMap::new(),
        }
    }

    /// Convert to a serde_json::Value for expression evaluation.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
    }

    /// Extract the short ref name from a full ref path.
    ///
    /// `refs/heads/main` → `main`
    /// `refs/tags/v1.0` → `v1.0`
    fn extract_ref_name(git_ref: &str) -> String {
        if let Some(name) = git_ref.strip_prefix("refs/heads/") {
            name.to_string()
        } else if let Some(name) = git_ref.strip_prefix("refs/tags/") {
            name.to_string()
        } else if let Some(name) = git_ref.strip_prefix("refs/pull/") {
            name.to_string()
        } else {
            git_ref.to_string()
        }
    }

    /// Determine if the ref is a branch or tag.
    fn extract_ref_type(git_ref: &str) -> String {
        if git_ref.starts_with("refs/heads/") {
            "branch".to_string()
        } else if git_ref.starts_with("refs/tags/") {
            "tag".to_string()
        } else {
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_ref_name_branch() {
        assert_eq!(
            GitHubContext::extract_ref_name("refs/heads/main"),
            "main"
        );
    }

    #[test]
    fn test_extract_ref_name_tag() {
        assert_eq!(
            GitHubContext::extract_ref_name("refs/tags/v1.0.0"),
            "v1.0.0"
        );
    }

    #[test]
    fn test_extract_ref_type() {
        assert_eq!(
            GitHubContext::extract_ref_type("refs/heads/main"),
            "branch"
        );
        assert_eq!(
            GitHubContext::extract_ref_type("refs/tags/v1.0"),
            "tag"
        );
        assert_eq!(
            GitHubContext::extract_ref_type("unknown"),
            ""
        );
    }

    #[test]
    fn test_to_value() {
        let ctx = GitHubContext {
            workflow: "CI".to_string(),
            repository: "owner/repo".to_string(),
            sha: "abc123".to_string(),
            ..Default::default()
        };

        let val = ctx.to_value();
        assert_eq!(val.get("workflow").unwrap().as_str(), Some("CI"));
        assert_eq!(val.get("repository").unwrap().as_str(), Some("owner/repo"));
        assert_eq!(val.get("sha").unwrap().as_str(), Some("abc123"));
    }
}
