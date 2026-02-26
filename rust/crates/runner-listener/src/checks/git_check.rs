// Git binary availability check.
// Maps to the C# Runner.Listener/Checks/GitCheck.cs.

use super::check_extension::CheckResult;
use std::process::Command;

const CHECK_NAME: &str = "Git";
const CHECK_DESCRIPTION: &str = "Check if git is installed and accessible";
const DOC_URL: &str = "https://github.com/actions/runner/blob/main/docs/checks/git.md";

pub struct GitCheck;

impl GitCheck {
    /// Run the Git availability check.
    pub async fn run_check() -> CheckResult {
        match Self::check_git() {
            Ok(_version) => CheckResult::pass(CHECK_NAME, CHECK_DESCRIPTION)
                .with_doc_url(DOC_URL),
            Err(e) => CheckResult::fail(CHECK_NAME, CHECK_DESCRIPTION, e.to_string())
                .with_doc_url(DOC_URL),
        }
    }

    fn check_git() -> Result<String, anyhow::Error> {
        // First check bundled git (externals/git)
        let externals_git = Self::find_externals_git();

        let git_path = if let Some(ref path) = externals_git {
            path.as_str()
        } else {
            "git"
        };

        let output = Command::new(git_path)
            .arg("--version")
            .output()
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to execute '{}': {}. Is git installed and on the PATH?",
                    git_path,
                    e
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!(
                "git --version exited with code {}: {}",
                output.status.code().unwrap_or(-1),
                stderr
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    }

    fn find_externals_git() -> Option<String> {
        // Try to find git in the externals directory relative to the runner binary
        if let Ok(exe) = std::env::current_exe() {
            if let Some(bin_dir) = exe.parent() {
                let externals_dir = bin_dir.join("externals").join("git");

                // Linux/macOS
                let git_bin = externals_dir.join("bin").join("git");
                if git_bin.exists() {
                    return Some(git_bin.to_string_lossy().to_string());
                }

                // Windows
                let git_cmd = externals_dir.join("cmd").join("git.exe");
                if git_cmd.exists() {
                    return Some(git_cmd.to_string_lossy().to_string());
                }
            }
        }
        None
    }
}

/// Check git LFS availability.
pub fn check_git_lfs() -> CheckResult {
    match Command::new("git").args(["lfs", "version"]).output() {
        Ok(output) if output.status.success() => {
            CheckResult::pass("Git LFS", "Check if git-lfs is installed")
                .with_doc_url(DOC_URL)
        }
        Ok(output) => CheckResult::fail(
            "Git LFS",
            "Check if git-lfs is installed",
            format!(
                "git lfs version failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        )
        .with_doc_url(DOC_URL),
        Err(e) => CheckResult::fail(
            "Git LFS",
            "Check if git-lfs is installed",
            format!("git-lfs not found: {}", e),
        )
        .with_doc_url(DOC_URL),
    }
}
