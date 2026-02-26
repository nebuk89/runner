// Node.js availability check.
// Maps to the C# Runner.Listener/Checks/NodeJsCheck.cs.
//
// Verifies that Node.js is available for running JavaScript/TypeScript actions.

use super::check_extension::CheckResult;
use std::process::Command;

const CHECK_NAME: &str = "Node.js";
const CHECK_DESCRIPTION: &str = "Check if Node.js is installed and accessible for running actions";
const DOC_URL: &str = "https://github.com/actions/runner/blob/main/docs/checks/nodejs.md";

/// Minimum required Node.js versions bundled with the runner.
const EXPECTED_NODE_VERSIONS: &[&str] = &["node16", "node20"];

pub struct NodeJsCheck;

impl NodeJsCheck {
    /// Run the Node.js availability check.
    pub async fn run_check() -> CheckResult {
        match Self::check_node() {
            Ok(detail) => {
                let mut result = CheckResult::pass(CHECK_NAME, CHECK_DESCRIPTION)
                    .with_doc_url(DOC_URL);
                result.detail = Some(detail);
                result
            }
            Err(e) => {
                CheckResult::fail(CHECK_NAME, CHECK_DESCRIPTION, e.to_string())
                    .with_doc_url(DOC_URL)
            }
        }
    }

    fn check_node() -> Result<String, anyhow::Error> {
        let mut found_versions = Vec::new();
        let mut missing_versions = Vec::new();

        // Check bundled node versions in externals directory
        if let Ok(exe) = std::env::current_exe() {
            if let Some(bin_dir) = exe.parent() {
                let externals_dir = bin_dir.join("externals");

                for version_dir in EXPECTED_NODE_VERSIONS {
                    let node_dir = externals_dir.join(version_dir);

                    let node_bin = if cfg!(windows) {
                        node_dir.join("bin").join("node.exe")
                    } else {
                        node_dir.join("bin").join("node")
                    };

                    if node_bin.exists() {
                        match Self::get_node_version(&node_bin) {
                            Ok(ver) => {
                                found_versions.push(format!(
                                    "{}: {}",
                                    version_dir, ver
                                ));
                            }
                            Err(e) => {
                                missing_versions.push(format!(
                                    "{}: exists but failed to get version: {}",
                                    version_dir, e
                                ));
                            }
                        }
                    } else {
                        missing_versions.push(format!(
                            "{}: not found at {}",
                            version_dir,
                            node_bin.display()
                        ));
                    }
                }
            }
        }

        // Fallback: check system node
        if found_versions.is_empty() {
            match Self::get_node_version(std::path::Path::new("node")) {
                Ok(ver) => {
                    found_versions.push(format!("system: {}", ver));
                }
                Err(_) => {
                    // Also try nodejs (some Linux distros)
                    if let Ok(ver) =
                        Self::get_node_version(std::path::Path::new("nodejs"))
                    {
                        found_versions.push(format!("system: {}", ver));
                    }
                }
            }
        }

        if found_versions.is_empty() {
            if missing_versions.is_empty() {
                Err(anyhow::anyhow!(
                    "Node.js not found. Neither bundled nor system Node.js is available."
                ))
            } else {
                Err(anyhow::anyhow!(
                    "Node.js not available: {}",
                    missing_versions.join("; ")
                ))
            }
        } else {
            let mut detail = format!("Found: {}", found_versions.join(", "));
            if !missing_versions.is_empty() {
                detail.push_str(&format!(
                    " (missing: {})",
                    missing_versions.join(", ")
                ));
            }
            Ok(detail)
        }
    }

    fn get_node_version(
        node_path: &std::path::Path,
    ) -> Result<String, anyhow::Error> {
        let output = Command::new(node_path)
            .arg("--version")
            .output()
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to run {}: {}",
                    node_path.display(),
                    e
                )
            })?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "{} --version failed with status {}",
                node_path.display(),
                output.status
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    }
}
