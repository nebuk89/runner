// FeatureManager mapping feature flag management in `Constants.cs`.
// Checks feature flags from the job message variables.

use std::collections::HashMap;

use crate::worker::AgentJobRequestMessage;

/// Manages feature flags for the current job.
///
/// Feature flags are passed as variables in the job message with a
/// specific prefix. This manager provides a clean API to check if
/// a feature is enabled.
pub struct FeatureManager {
    /// Map of feature flag name → enabled.
    features: HashMap<String, bool>,
}

impl FeatureManager {
    /// Create a `FeatureManager` from a job message.
    ///
    /// Extracts feature flags from the message variables.
    /// Feature flags are variables whose names start with the feature flag prefix.
    pub fn new(message: &AgentJobRequestMessage) -> Self {
        let mut features = HashMap::new();

        for (var_name, var_value) in &message.variables {
            // Feature flags start with specific prefixes
            let name_lower = var_name.to_lowercase();

            // Check for "system.runner.features." prefix (GitHub Actions convention)
            if let Some(flag) = name_lower.strip_prefix("system.runner.features.") {
                let enabled = var_value.value.eq_ignore_ascii_case("true")
                    || var_value.value == "1";
                features.insert(flag.to_string(), enabled);
            }

            // Also check "actions.runner." prefix
            if let Some(flag) = name_lower.strip_prefix("actions.runner.") {
                let enabled = var_value.value.eq_ignore_ascii_case("true")
                    || var_value.value == "1";
                features.insert(flag.to_string(), enabled);
            }

            // Check for DistributedTask feature flags
            if let Some(flag) = name_lower.strip_prefix("distributedtask.") {
                let enabled = var_value.value.eq_ignore_ascii_case("true")
                    || var_value.value == "1";
                features.insert(flag.to_string(), enabled);
            }
        }

        // Also check environment variables for feature flags
        for (key, value) in std::env::vars() {
            let key_lower = key.to_lowercase();
            if let Some(flag) = key_lower.strip_prefix("actions_runner_feature_") {
                let enabled = value.eq_ignore_ascii_case("true") || value == "1";
                features.insert(flag.to_string(), enabled);
            }
        }

        Self { features }
    }

    /// Create an empty `FeatureManager` with no features enabled.
    pub fn empty() -> Self {
        Self {
            features: HashMap::new(),
        }
    }

    /// Check if a feature flag is enabled.
    ///
    /// The flag name is case-insensitive.
    pub fn is_feature_enabled(&self, flag: &str) -> bool {
        let lower = flag.to_lowercase();
        self.features.get(&lower).copied().unwrap_or(false)
    }

    /// Check if the "debug" feature is enabled (ACTIONS_STEP_DEBUG).
    pub fn is_debug_enabled(&self) -> bool {
        std::env::var("ACTIONS_STEP_DEBUG")
            .or_else(|_| std::env::var("ACTIONS_RUNNER_DEBUG"))
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    }

    /// Check if Node20 force is enabled.
    pub fn is_force_node20(&self) -> bool {
        self.is_feature_enabled("forceactionsnode20")
    }

    /// Check if Node20 to Node24 migration warnings are enabled.
    pub fn is_node24_migration_warning(&self) -> bool {
        self.is_feature_enabled("node24migrationwarning")
    }

    /// Get all enabled feature flags.
    pub fn enabled_features(&self) -> Vec<String> {
        self.features
            .iter()
            .filter(|(_, enabled)| **enabled)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Get the total number of known feature flags.
    pub fn total_features(&self) -> usize {
        self.features.len()
    }
}

impl Default for FeatureManager {
    fn default() -> Self {
        Self::empty()
    }
}

impl Clone for FeatureManager {
    fn clone(&self) -> Self {
        Self {
            features: self.features.clone(),
        }
    }
}

impl std::fmt::Debug for FeatureManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureManager")
            .field("enabled", &self.enabled_features())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::VariableValueMessage;

    fn make_message(variables: Vec<(&str, &str)>) -> AgentJobRequestMessage {
        AgentJobRequestMessage {
            message_type: "PipelineAgentJobRequest".to_string(),
            job_id: String::new(),
            job_display_name: String::new(),
            request_id: 0,
            plan_id: String::new(),
            timeline_id: String::new(),
            environment_variables: Default::default(),
            variables: variables
                .into_iter()
                .map(|(k, v)| (k.to_string(), VariableValueMessage {
                    value: v.to_string(),
                    is_secret: false,
                    is_read_only: false,
                }))
                .collect(),
            steps: vec![],
            resources: Default::default(),
            workspace: None,
            file_table: vec![],
            context_data: Default::default(),
            job_container: None,
            job_service_containers: Default::default(),
            actor: String::new(),
        }
    }

    #[test]
    fn test_empty_features() {
        let fm = FeatureManager::empty();
        assert!(!fm.is_feature_enabled("anything"));
        assert_eq!(fm.total_features(), 0);
    }

    #[test]
    fn test_feature_from_message() {
        let msg = make_message(vec![
            ("system.runner.features.testfeature", "true"),
            ("system.runner.features.disabledfeature", "false"),
        ]);

        let fm = FeatureManager::new(&msg);
        assert!(fm.is_feature_enabled("testfeature"));
        assert!(!fm.is_feature_enabled("disabledfeature"));
    }

    #[test]
    fn test_case_insensitive() {
        let msg = make_message(vec![
            ("system.runner.features.MyFeature", "true"),
        ]);

        let fm = FeatureManager::new(&msg);
        assert!(fm.is_feature_enabled("myfeature"));
        assert!(fm.is_feature_enabled("MYFEATURE"));
    }

    #[test]
    fn test_enabled_features_list() {
        let msg = make_message(vec![
            ("system.runner.features.a", "true"),
            ("system.runner.features.b", "false"),
            ("system.runner.features.c", "true"),
        ]);

        let fm = FeatureManager::new(&msg);
        let enabled = fm.enabled_features();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.contains(&"a".to_string()));
        assert!(enabled.contains(&"c".to_string()));
    }

    #[test]
    fn test_clone() {
        let fm = FeatureManager::empty();
        let fm2 = fm.clone();
        assert_eq!(fm2.total_features(), 0);
    }
}
