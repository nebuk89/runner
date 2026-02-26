// NodeUtil mapping `Util/NodeUtil.cs`.
// Node.js version resolution for the node20 → node24 migration.

use crate::constants::{self, Architecture, OsPlatform, CURRENT_ARCHITECTURE, CURRENT_PLATFORM};
use runner_sdk::StringUtil;
use std::collections::HashMap;

/// Built-in Node.js versions bundled with the runner.
pub const BUILT_IN_NODE_VERSIONS: &[&str] = &["node20"];

/// Default Node.js version used for internal runner functions.
const DEFAULT_NODE_VERSION: &str = "node20";

/// Node.js version resolution utilities.
pub struct NodeUtil;

/// Details about an environment variable lookup, including its source.
struct EnvironmentVariableInfo {
    is_true: bool,
    from_workflow: bool,
    from_system: bool,
}

impl NodeUtil {
    /// Get the internal Node version used for runner-internal functions (e.g. hashFiles).
    ///
    /// Checks `ACTIONS_RUNNER_FORCED_INTERNAL_NODE_VERSION` to allow overriding.
    pub fn get_internal_node_version() -> &'static str {
        if let Ok(forced) =
            std::env::var(constants::variables::agent::FORCED_INTERNAL_NODE_VERSION)
        {
            if !forced.is_empty() && BUILT_IN_NODE_VERSIONS.contains(&forced.as_str()) {
                // Leak the string so we can return a &'static str
                // (In practice this is called rarely and the value is small)
                return Box::leak(forced.into_boxed_str());
            }
        }
        DEFAULT_NODE_VERSION
    }

    /// Determine the appropriate Node version for Actions to use.
    ///
    /// Returns `(node_version, optional_warning_message)`.
    ///
    /// The migration phases are:
    /// - Phase 3 (`require_node24`): Always use Node 24
    /// - Phase 2 (`use_node24_by_default`): Node 24 by default, allow opt-out
    /// - Phase 1 (default): Node 20 by default, allow opt-in to Node 24
    pub fn determine_actions_node_version(
        workflow_environment: Option<&HashMap<String, String>>,
        use_node24_by_default: bool,
        require_node24: bool,
    ) -> (String, Option<String>) {
        // Phase 3: Always use Node 24
        if require_node24 {
            return (constants::node_migration::NODE24.to_string(), None);
        }

        // Get environment variable details with source information
        let force_node24_details = Self::get_env_var_details(
            constants::node_migration::FORCE_NODE24_VARIABLE,
            workflow_environment,
        );

        let allow_unsecure_details = Self::get_env_var_details(
            constants::node_migration::ALLOW_UNSECURE_NODE_VERSION_VARIABLE,
            workflow_environment,
        );

        let force_node24 = force_node24_details.is_true;
        let allow_unsecure_node = allow_unsecure_details.is_true;

        // Check if both flags are set from the same source
        let both_from_workflow = force_node24_details.is_true
            && allow_unsecure_details.is_true
            && force_node24_details.from_workflow
            && allow_unsecure_details.from_workflow;

        let both_from_system = force_node24_details.is_true
            && allow_unsecure_details.is_true
            && force_node24_details.from_system
            && allow_unsecure_details.from_system;

        // Handle the case when both are set in the same source
        if both_from_workflow || both_from_system {
            let source = if both_from_workflow {
                "workflow"
            } else {
                "system"
            };
            let default_version = if use_node24_by_default {
                constants::node_migration::NODE24
            } else {
                constants::node_migration::NODE20
            };
            let warning = format!(
                "Both {} and {} environment variables are set to true in the {} environment. \
                 This is likely a configuration error. Using the default Node version: {}.",
                constants::node_migration::FORCE_NODE24_VARIABLE,
                constants::node_migration::ALLOW_UNSECURE_NODE_VERSION_VARIABLE,
                source,
                default_version
            );
            return (default_version.to_string(), Some(warning));
        }

        // Phase 2: Node 24 is the default
        if use_node24_by_default {
            if allow_unsecure_node {
                return (constants::node_migration::NODE20.to_string(), None);
            }
            return (constants::node_migration::NODE24.to_string(), None);
        }

        // Phase 1: Node 20 is the default
        if force_node24 {
            return (constants::node_migration::NODE24.to_string(), None);
        }

        (constants::node_migration::NODE20.to_string(), None)
    }

    /// Check if Node 24 is requested but running on ARM32 Linux,
    /// and determine if fallback is needed.
    ///
    /// Returns `(adjusted_node_version, optional_warning_message)`.
    pub fn check_node_version_for_linux_arm32(
        preferred_version: &str,
    ) -> (String, Option<String>) {
        if preferred_version.eq_ignore_ascii_case(constants::node_migration::NODE24)
            && CURRENT_ARCHITECTURE == Architecture::Arm
            && CURRENT_PLATFORM == OsPlatform::Linux
        {
            return (
                constants::node_migration::NODE20.to_string(),
                Some(
                    "Node 24 is not supported on Linux ARM32 platforms. Falling back to Node 20."
                        .to_string(),
                ),
            );
        }

        (preferred_version.to_string(), None)
    }

    /// Get detailed information about an environment variable from both workflow and system environments.
    fn get_env_var_details(
        variable_name: &str,
        workflow_environment: Option<&HashMap<String, String>>,
    ) -> EnvironmentVariableInfo {
        let mut info = EnvironmentVariableInfo {
            is_true: false,
            from_workflow: false,
            from_system: false,
        };

        // Check workflow environment
        let mut found_in_workflow = false;
        if let Some(env) = workflow_environment {
            if let Some(value) = env.get(variable_name) {
                found_in_workflow = true;
                info.from_workflow = true;
                info.is_true = StringUtil::convert_to_bool(value) == Some(true);
            }
        }

        // Check system environment
        if let Ok(system_value) = std::env::var(variable_name) {
            if !system_value.is_empty() {
                info.from_system = true;

                // If not found in workflow, use system value
                if !found_in_workflow {
                    info.is_true = StringUtil::convert_to_bool(&system_value) == Some(true);
                }
            }
        }

        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_internal_node_version_default() {
        // Without env var override, should return default
        let version = NodeUtil::get_internal_node_version();
        assert!(!version.is_empty());
    }

    #[test]
    fn test_determine_actions_node_version_default() {
        let (version, warning) = NodeUtil::determine_actions_node_version(None, false, false);
        assert_eq!(version, "node20");
        assert!(warning.is_none());
    }

    #[test]
    fn test_determine_actions_node_version_require_node24() {
        let (version, warning) = NodeUtil::determine_actions_node_version(None, false, true);
        assert_eq!(version, "node24");
        assert!(warning.is_none());
    }

    #[test]
    fn test_determine_actions_node_version_use_node24_by_default() {
        let (version, warning) = NodeUtil::determine_actions_node_version(None, true, false);
        assert_eq!(version, "node24");
        assert!(warning.is_none());
    }

    #[test]
    fn test_check_node_version_no_fallback_on_x64() {
        let (version, warning) = NodeUtil::check_node_version_for_linux_arm32("node24");
        // On x64 (or any non-ARM32 Linux), no fallback should happen
        if CURRENT_ARCHITECTURE != Architecture::Arm || CURRENT_PLATFORM != OsPlatform::Linux {
            assert_eq!(version, "node24");
            assert!(warning.is_none());
        }
    }
}
