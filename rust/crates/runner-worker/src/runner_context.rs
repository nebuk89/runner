// RunnerContext mapping `RunnerContext.cs`.
// Populates the `runner.*` expression context from the host environment.

use std::collections::HashMap;

use runner_common::constants;

/// The `runner` context available in expressions.
///
/// Contains information about the runner environment: OS, architecture,
/// tool cache, temp directory, and debug mode.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RunnerContext {
    /// The runner OS: "Linux", "Windows", or "macOS".
    pub os: String,

    /// The runner architecture: "X64", "ARM64", "ARM", etc.
    pub arch: String,

    /// The runner name.
    pub name: String,

    /// Path to the tool cache directory.
    pub tool_cache: String,

    /// Path to the runner temp directory.
    pub temp: String,

    /// Whether debug mode is enabled.
    pub debug: String,

    /// The runner workspace directory.
    pub workspace: String,

    /// The runner environment: "github-hosted" or "self-hosted".
    pub environment: String,
}

impl RunnerContext {
    /// Create a `RunnerContext` from the current host environment.
    pub fn from_environment() -> Self {
        let os = Self::detect_os();
        let arch = Self::detect_arch();

        let name = std::env::var("RUNNER_NAME").unwrap_or_else(|_| {
            hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        });

        let tool_cache = std::env::var("RUNNER_TOOL_CACHE")
            .or_else(|_| std::env::var("AGENT_TOOLSDIRECTORY"))
            .unwrap_or_default();

        let temp = std::env::var("RUNNER_TEMP").unwrap_or_else(|_| {
            std::env::temp_dir().to_string_lossy().to_string()
        });

        let debug = if std::env::var("ACTIONS_RUNNER_DEBUG")
            .or_else(|_| std::env::var("ACTIONS_STEP_DEBUG"))
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            "1".to_string()
        } else {
            String::new()
        };

        let workspace = std::env::var("GITHUB_WORKSPACE").unwrap_or_default();

        let environment = if std::env::var("RUNNER_ENVIRONMENT")
            .unwrap_or_default()
            .eq_ignore_ascii_case("github-hosted")
        {
            "github-hosted".to_string()
        } else {
            "self-hosted".to_string()
        };

        Self {
            os,
            arch,
            name,
            tool_cache,
            temp,
            debug,
            workspace,
            environment,
        }
    }

    /// Create a `RunnerContext` with custom values (for testing or injection).
    pub fn with_values(
        name: &str,
        workspace: &str,
        temp: &str,
        tool_cache: &str,
        debug: bool,
    ) -> Self {
        Self {
            os: Self::detect_os(),
            arch: Self::detect_arch(),
            name: name.to_string(),
            tool_cache: tool_cache.to_string(),
            temp: temp.to_string(),
            debug: if debug { "1".to_string() } else { String::new() },
            workspace: workspace.to_string(),
            environment: "self-hosted".to_string(),
        }
    }

    /// Convert to a serde_json::Value for expression evaluation.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
    }

    /// Detect the current OS as a GitHub Actions-compatible string.
    fn detect_os() -> String {
        match constants::CURRENT_PLATFORM {
            constants::OsPlatform::Linux => "Linux".to_string(),
            constants::OsPlatform::MacOS => "macOS".to_string(),
            constants::OsPlatform::Windows => "Windows".to_string(),
        }
    }

    /// Detect the current CPU architecture.
    fn detect_arch() -> String {
        match std::env::consts::ARCH {
            "x86_64" => "X64".to_string(),
            "aarch64" => "ARM64".to_string(),
            "arm" => "ARM".to_string(),
            "x86" => "X86".to_string(),
            other => other.to_uppercase(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_os() {
        let os = RunnerContext::detect_os();
        assert!(
            os == "Linux" || os == "macOS" || os == "Windows",
            "Unexpected OS: {}",
            os
        );
    }

    #[test]
    fn test_detect_arch() {
        let arch = RunnerContext::detect_arch();
        // Should be a non-empty string
        assert!(!arch.is_empty());
    }

    #[test]
    fn test_with_values() {
        let ctx = RunnerContext::with_values(
            "my-runner",
            "/home/runner/work",
            "/tmp/runner",
            "/opt/hostedtoolcache",
            true,
        );

        assert_eq!(ctx.name, "my-runner");
        assert_eq!(ctx.workspace, "/home/runner/work");
        assert_eq!(ctx.temp, "/tmp/runner");
        assert_eq!(ctx.tool_cache, "/opt/hostedtoolcache");
        assert_eq!(ctx.debug, "1");
    }

    #[test]
    fn test_to_value() {
        let ctx = RunnerContext::with_values(
            "test-runner",
            "/work",
            "/tmp",
            "/tools",
            false,
        );

        let val = ctx.to_value();
        assert_eq!(val.get("name").unwrap().as_str(), Some("test-runner"));
        assert_eq!(val.get("workspace").unwrap().as_str(), Some("/work"));
        assert_eq!(val.get("temp").unwrap().as_str(), Some("/tmp"));
    }

    #[test]
    fn test_debug_flag() {
        let ctx_debug = RunnerContext::with_values("r", "", "", "", true);
        assert_eq!(ctx_debug.debug, "1");

        let ctx_no_debug = RunnerContext::with_values("r", "", "", "", false);
        assert_eq!(ctx_no_debug.debug, "");
    }
}
