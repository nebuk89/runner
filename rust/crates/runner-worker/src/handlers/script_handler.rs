// ScriptHandler mapping `ScriptHandler.cs`.
// Executes inline `run:` scripts by writing them to a temp file and
// invoking them via the appropriate shell.

use async_trait::async_trait;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::execution_context::ExecutionContext;
use crate::handlers::handler::{Handler, HandlerData};
use crate::handlers::step_host::{DefaultStepHost, StepHost};

/// Script handler for `run:` steps.
pub struct ScriptHandler;

impl ScriptHandler {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Handler for ScriptHandler {
    async fn run_async(
        &self,
        context: &mut ExecutionContext,
        data: &HandlerData,
    ) -> Result<()> {
        self.prepare_execution(context, data);

        // Get the script body
        let script = data
            .inputs
            .get("script")
            .cloned()
            .unwrap_or_default();

        if script.trim().is_empty() {
            context.debug("Script body is empty, skipping.");
            return Ok(());
        }

        // Determine shell
        let shell = data
            .inputs
            .get("shell")
            .cloned()
            .unwrap_or_else(|| ScriptHandlerHelpers::get_default_shell());

        // Parse shell options
        let (shell_command, shell_args, file_extension) =
            ScriptHandlerHelpers::parse_shell_option_string(&shell);

        // Write the script to a temp file
        let temp_dir = context.global().temp_directory.clone();
        let script_file = format!(
            "{}/script_{}.{}",
            temp_dir,
            uuid::Uuid::new_v4().as_simple(),
            file_extension
        );

        // Ensure temp directory exists
        let _ = std::fs::create_dir_all(&temp_dir);

        std::fs::write(&script_file, &script)
            .with_context(|| format!("Failed to write script file: {}", script_file))?;

        // Make the script executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            let _ = std::fs::set_permissions(&script_file, perms);
        }

        context.debug(&format!("Script file: {}", script_file));
        context.debug(&format!("Shell: {} {}", shell_command, shell_args.join(" ")));

        // Build the final command arguments
        let mut args = shell_args.clone();
        args.push(script_file.clone());

        let arguments = args.join(" ");

        // Build environment
        let mut env = context.global().environment_variables.clone();
        for (k, v) in &context.step_environment {
            env.insert(k.clone(), v.clone());
        }

        // Prepend paths
        let prepend = context.global().prepend_path.clone();
        if !prepend.is_empty() {
            let current_path = env
                .get(runner_common::constants::PATH_VARIABLE)
                .cloned()
                .or_else(|| std::env::var(runner_common::constants::PATH_VARIABLE).ok())
                .unwrap_or_default();

            let separator = if cfg!(windows) { ";" } else { ":" };
            let new_path = format!("{}{}{}", prepend.join(separator), separator, current_path);
            env.insert(
                runner_common::constants::PATH_VARIABLE.to_string(),
                new_path,
            );
        }

        // Determine working directory
        let working_directory = data
            .inputs
            .get("working-directory")
            .cloned()
            .unwrap_or_else(|| context.global().workspace_directory.clone());

        // Execute via StepHost
        let step_host = DefaultStepHost::new();

        let step_output = step_host
            .execute_async(
                &working_directory,
                &shell_command,
                &arguments,
                &env,
                context.cancel_token(),
            )
            .await?;

        // Feed captured output lines into the execution context's log
        for line in &step_output.output_lines {
            context.write(line);
        }

        // Clean up temp file
        let _ = std::fs::remove_file(&script_file);

        // Handle exit code
        if step_output.exit_code != 0 {
            context.error(&format!(
                "Process completed with exit code {}.",
                step_output.exit_code
            ));
            context.complete(
                runner_common::util::task_result_util::TaskResult::Failed,
                Some(&format!("Exit code {}", step_output.exit_code)),
            );
        } else {
            context.debug("Process completed successfully.");
        }

        context.end_section();

        Ok(())
    }
}

/// Helper functions for shell resolution and script file handling.
pub struct ScriptHandlerHelpers;

impl ScriptHandlerHelpers {
    /// Get the default shell for the current platform.
    pub fn get_default_shell() -> String {
        if cfg!(windows) {
            "pwsh".to_string()
        } else {
            "bash".to_string()
        }
    }

    /// Parse a shell option string into (command, args, file_extension).
    ///
    /// Supported shells:
    /// - `bash` → ("bash", ["--noprofile", "--norc", "-e", "-o", "pipefail", "{0}"], "sh")
    /// - `sh` → ("sh", ["-e", "{0}"], "sh")
    /// - `pwsh` → ("pwsh", ["-command", ". '{0}'"], "ps1")
    /// - `powershell` → ("powershell", ["-command", ". '{0}'"], "ps1")
    /// - `python` → ("python", ["{0}"], "py")
    /// - `cmd` → ("cmd", ["/D", "/E:ON", "/V:OFF", "/S", "/C", "call \"{0}\""], "cmd")
    /// - Custom → split on whitespace
    pub fn parse_shell_option_string(shell: &str) -> (String, Vec<String>, String) {
        match shell.to_lowercase().as_str() {
            "bash" => (
                "bash".to_string(),
                vec![
                    "--noprofile".to_string(),
                    "--norc".to_string(),
                    "-e".to_string(),
                    "-o".to_string(),
                    "pipefail".to_string(),
                ],
                "sh".to_string(),
            ),
            "sh" => (
                "sh".to_string(),
                vec!["-e".to_string()],
                "sh".to_string(),
            ),
            "pwsh" => (
                "pwsh".to_string(),
                vec!["-command".to_string(), ". ".to_string()],
                "ps1".to_string(),
            ),
            "powershell" => (
                "powershell".to_string(),
                vec!["-command".to_string(), ". ".to_string()],
                "ps1".to_string(),
            ),
            "python" => (
                "python3".to_string(),
                vec![],
                "py".to_string(),
            ),
            "cmd" => (
                "cmd".to_string(),
                vec![
                    "/D".to_string(),
                    "/E:ON".to_string(),
                    "/V:OFF".to_string(),
                    "/S".to_string(),
                    "/C".to_string(),
                    "call".to_string(),
                ],
                "cmd".to_string(),
            ),
            _ => {
                // Custom shell - split on whitespace
                let parts: Vec<&str> = shell.split_whitespace().collect();
                if parts.is_empty() {
                    ("bash".to_string(), vec![], "sh".to_string())
                } else {
                    let cmd = parts[0].to_string();
                    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
                    let ext = Self::get_script_file_extension(&cmd);
                    (cmd, args, ext)
                }
            }
        }
    }

    /// Get the script file extension for a given shell command.
    pub fn get_script_file_extension(shell: &str) -> String {
        let basename = Path::new(shell)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(shell);

        match basename.to_lowercase().as_str() {
            "bash" | "sh" | "zsh" => "sh".to_string(),
            "pwsh" | "powershell" => "ps1".to_string(),
            "python" | "python3" => "py".to_string(),
            "cmd" => "cmd".to_string(),
            "node" | "nodejs" => "js".to_string(),
            "ruby" => "rb".to_string(),
            "perl" => "pl".to_string(),
            _ => "sh".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bash_shell() {
        let (cmd, args, ext) = ScriptHandlerHelpers::parse_shell_option_string("bash");
        assert_eq!(cmd, "bash");
        assert!(args.contains(&"--noprofile".to_string()));
        assert_eq!(ext, "sh");
    }

    #[test]
    fn test_parse_pwsh_shell() {
        let (cmd, _args, ext) = ScriptHandlerHelpers::parse_shell_option_string("pwsh");
        assert_eq!(cmd, "pwsh");
        assert_eq!(ext, "ps1");
    }

    #[test]
    fn test_parse_python_shell() {
        let (cmd, _args, ext) = ScriptHandlerHelpers::parse_shell_option_string("python");
        assert_eq!(cmd, "python3");
        assert_eq!(ext, "py");
    }

    #[test]
    fn test_parse_custom_shell() {
        let (cmd, args, ext) = ScriptHandlerHelpers::parse_shell_option_string("/usr/bin/env ruby");
        assert_eq!(cmd, "/usr/bin/env");
        assert_eq!(args, vec!["ruby"]);
        assert_eq!(ext, "sh"); // defaults for unknown
    }

    #[test]
    fn test_get_script_file_extension() {
        assert_eq!(ScriptHandlerHelpers::get_script_file_extension("bash"), "sh");
        assert_eq!(ScriptHandlerHelpers::get_script_file_extension("pwsh"), "ps1");
        assert_eq!(ScriptHandlerHelpers::get_script_file_extension("python3"), "py");
        assert_eq!(ScriptHandlerHelpers::get_script_file_extension("node"), "js");
    }

    #[test]
    fn test_default_shell() {
        let shell = ScriptHandlerHelpers::get_default_shell();
        if cfg!(windows) {
            assert_eq!(shell, "pwsh");
        } else {
            assert_eq!(shell, "bash");
        }
    }
}
