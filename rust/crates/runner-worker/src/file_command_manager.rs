// FileCommandManager mapping `FileCommandManager.cs`.
// Manages file-based workflow commands (GITHUB_ENV, GITHUB_PATH, GITHUB_OUTPUT, etc.).
// Steps write to these files, and the manager processes them after each step.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};

use crate::execution_context::ExecutionContext;

/// Well-known file command names mapped to environment variable names.
const FILE_COMMANDS: &[(&str, &str)] = &[
    ("GITHUB_ENV", "GITHUB_ENV"),
    ("GITHUB_PATH", "GITHUB_PATH"),
    ("GITHUB_OUTPUT", "GITHUB_OUTPUT"),
    ("GITHUB_STEP_SUMMARY", "GITHUB_STEP_SUMMARY"),
    ("GITHUB_STATE", "GITHUB_STATE"),
];

/// Maximum summary size in kilobytes (1024 KB).
const MAX_SUMMARY_SIZE_KB: usize = 1024;

/// Manages file-based commands that steps use to communicate environment changes,
/// outputs, and summaries back to the runner.
pub struct FileCommandManager;

impl FileCommandManager {
    /// Initialize file command temp files for a step.
    ///
    /// Creates temporary files for each file command and sets the corresponding
    /// environment variable pointing to the file path.
    pub fn initialize_file_commands(context: &mut ExecutionContext) {
        let temp_dir = context.global().temp_directory.clone();

        for &(name, env_var) in FILE_COMMANDS {
            let file_path = format!(
                "{}/{}_{}.txt",
                temp_dir,
                name.to_lowercase(),
                uuid::Uuid::new_v4().as_simple()
            );

            // Create the file
            if let Err(e) = std::fs::write(&file_path, "") {
                context.warning(&format!(
                    "Failed to create file command file for {}: {}",
                    name, e
                ));
                continue;
            }

            // Store the path for later processing
            context.file_command_paths.insert(name.to_string(), file_path.clone());

            // Set the environment variable so the step knows where to write
            context
                .global_mut()
                .environment_variables
                .insert(env_var.to_string(), file_path);
        }
    }

    /// Process all file commands after a step completes.
    ///
    /// Reads the contents of each file command file and applies the
    /// corresponding changes to the execution context.
    pub fn process_file_commands(context: &mut ExecutionContext) {
        let paths = context.file_command_paths.clone();

        for (name, path) in &paths {
            match name.as_str() {
                "GITHUB_ENV" => Self::process_env_file(context, path),
                "GITHUB_PATH" => Self::process_path_file(context, path),
                "GITHUB_OUTPUT" => Self::process_output_file(context, path),
                "GITHUB_STEP_SUMMARY" => Self::process_summary_file(context, path),
                "GITHUB_STATE" => Self::process_state_file(context, path),
                _ => {
                    context.debug(&format!("Unknown file command: {}", name));
                }
            }

            // Clean up the temp file
            let _ = std::fs::remove_file(path);
        }

        context.file_command_paths.clear();
    }

    /// Process the GITHUB_ENV file – adds environment variables.
    ///
    /// Format is either:
    /// - `NAME=VALUE` (single line)
    /// - Multi-line heredoc: `NAME<<DELIMITER\nVALUE\nDELIMITER`
    fn process_env_file(context: &mut ExecutionContext, path: &str) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                context.debug(&format!("Failed to read GITHUB_ENV file: {}", e));
                return;
            }
        };

        if content.trim().is_empty() {
            return;
        }

        let mut lines = content.lines().peekable();

        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Check for heredoc format: NAME<<DELIMITER
            if let Some(heredoc_pos) = line.find("<<") {
                let name = line[..heredoc_pos].trim().to_string();
                let delimiter = line[heredoc_pos + 2..].trim().to_string();

                if name.is_empty() || delimiter.is_empty() {
                    context.warning(&format!("Invalid heredoc format in GITHUB_ENV: {}", line));
                    continue;
                }

                let mut value_lines = Vec::new();
                while let Some(val_line) = lines.next() {
                    if val_line.trim() == delimiter {
                        break;
                    }
                    value_lines.push(val_line);
                }
                let value = value_lines.join("\n");

                context.debug(&format!("GITHUB_ENV: {}={}", name, value));
                context.global_mut().environment_variables.insert(name, value);
            } else if let Some(eq_pos) = line.find('=') {
                // Simple KEY=VALUE format
                let name = line[..eq_pos].trim().to_string();
                let value = line[eq_pos + 1..].trim().to_string();

                if name.is_empty() {
                    context.warning(&format!("Invalid env entry (empty name): {}", line));
                    continue;
                }

                context.debug(&format!("GITHUB_ENV: {}={}", name, value));
                context.global_mut().environment_variables.insert(name, value);
            } else {
                context.warning(&format!("Unrecognized GITHUB_ENV line: {}", line));
            }
        }
    }

    /// Process the GITHUB_PATH file – prepends paths.
    fn process_path_file(context: &mut ExecutionContext, path: &str) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                context.debug(&format!("Failed to read GITHUB_PATH file: {}", e));
                return;
            }
        };

        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                context.debug(&format!("GITHUB_PATH: prepending {}", trimmed));
                context.global_mut().prepend_path.push(trimmed.to_string());
            }
        }
    }

    /// Process the GITHUB_OUTPUT file – sets step outputs.
    ///
    /// Same format as GITHUB_ENV (KEY=VALUE or heredoc).
    fn process_output_file(context: &mut ExecutionContext, path: &str) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                context.debug(&format!("Failed to read GITHUB_OUTPUT file: {}", e));
                return;
            }
        };

        if content.trim().is_empty() {
            return;
        }

        let mut lines = content.lines().peekable();

        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(heredoc_pos) = line.find("<<") {
                let name = line[..heredoc_pos].trim().to_string();
                let delimiter = line[heredoc_pos + 2..].trim().to_string();

                let mut value_lines = Vec::new();
                while let Some(val_line) = lines.next() {
                    if val_line.trim() == delimiter {
                        break;
                    }
                    value_lines.push(val_line);
                }
                let value = value_lines.join("\n");

                context.debug(&format!("GITHUB_OUTPUT: {}={}", name, value));
                context.outputs.insert(name, value);
            } else if let Some(eq_pos) = line.find('=') {
                let name = line[..eq_pos].trim().to_string();
                let value = line[eq_pos + 1..].trim().to_string();

                context.debug(&format!("GITHUB_OUTPUT: {}={}", name, value));
                context.outputs.insert(name, value);
            }
        }
    }

    /// Process the GITHUB_STEP_SUMMARY file.
    fn process_summary_file(context: &mut ExecutionContext, path: &str) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                context.debug(&format!("Failed to read GITHUB_STEP_SUMMARY file: {}", e));
                return;
            }
        };

        if content.trim().is_empty() {
            return;
        }

        let size_kb = content.len() / 1024;
        if size_kb > MAX_SUMMARY_SIZE_KB {
            context.warning(&format!(
                "Step summary is too large ({} KB, max {} KB). Summary will be truncated.",
                size_kb, MAX_SUMMARY_SIZE_KB
            ));
        }

        context.debug(&format!(
            "GITHUB_STEP_SUMMARY: {} bytes processed",
            content.len()
        ));
    }

    /// Process the GITHUB_STATE file – saves state for post steps.
    fn process_state_file(context: &mut ExecutionContext, path: &str) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                context.debug(&format!("Failed to read GITHUB_STATE file: {}", e));
                return;
            }
        };

        if content.trim().is_empty() {
            return;
        }

        let mut lines = content.lines().peekable();

        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(heredoc_pos) = line.find("<<") {
                let name = line[..heredoc_pos].trim().to_string();
                let delimiter = line[heredoc_pos + 2..].trim().to_string();

                let mut value_lines = Vec::new();
                while let Some(val_line) = lines.next() {
                    if val_line.trim() == delimiter {
                        break;
                    }
                    value_lines.push(val_line);
                }
                let value = value_lines.join("\n");

                context.debug(&format!("GITHUB_STATE: {}={}", name, value));
                context.outputs.insert(format!("STATE_{}", name), value);
            } else if let Some(eq_pos) = line.find('=') {
                let name = line[..eq_pos].trim().to_string();
                let value = line[eq_pos + 1..].trim().to_string();

                context.debug(&format!("GITHUB_STATE: {}={}", name, value));
                context.outputs.insert(format!("STATE_{}", name), value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_context::Global;
    use crate::feature_manager::FeatureManager;
    use crate::variables::Variables;
    use runner_common::host_context::HostContext;
    use tokio_util::sync::CancellationToken;

    fn make_ctx() -> ExecutionContext {
        let host = HostContext::new("Test");
        let global = Global {
            variables: Variables::new(),
            endpoints: Vec::new(),
            file_table: Vec::new(),
            environment_variables: HashMap::new(),
            job_display_name: "test".to_string(),
            job_id: "j1".to_string(),
            plan_id: "p1".to_string(),
            timeline_id: "t1".to_string(),
            pipeline_directory: "/tmp".to_string(),
            workspace_directory: "/tmp/w".to_string(),
            temp_directory: std::env::temp_dir().to_string_lossy().to_string(),
            prepend_path: Vec::new(),
            container_info: None,
            service_containers: Vec::new(),
            job_telemetry: Vec::new(),
            environment_url: None,
            cancel_token: CancellationToken::new(),
            feature_manager: FeatureManager::empty(),
            write_debug: true,
        };
        ExecutionContext::new_root(host, global, "test".to_string())
    }

    #[test]
    fn test_process_env_file_simple() {
        let mut ctx = make_ctx();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "MY_VAR=hello\nOTHER=world\n").unwrap();

        FileCommandManager::process_env_file(
            &mut ctx,
            tmp.path().to_str().unwrap(),
        );

        let global = ctx.global();
        assert_eq!(global.environment_variables.get("MY_VAR"), Some(&"hello".to_string()));
        assert_eq!(global.environment_variables.get("OTHER"), Some(&"world".to_string()));
    }

    #[test]
    fn test_process_env_file_heredoc() {
        let mut ctx = make_ctx();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "MY_VAR<<EOF\nline1\nline2\nEOF\n").unwrap();

        FileCommandManager::process_env_file(
            &mut ctx,
            tmp.path().to_str().unwrap(),
        );

        let global = ctx.global();
        assert_eq!(
            global.environment_variables.get("MY_VAR"),
            Some(&"line1\nline2".to_string())
        );
    }

    #[test]
    fn test_process_path_file() {
        let mut ctx = make_ctx();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "/usr/local/bin\n/opt/custom\n").unwrap();

        FileCommandManager::process_path_file(
            &mut ctx,
            tmp.path().to_str().unwrap(),
        );

        let global = ctx.global();
        assert_eq!(global.prepend_path.len(), 2);
        assert_eq!(global.prepend_path[0], "/usr/local/bin");
    }

    #[test]
    fn test_process_output_file() {
        let mut ctx = make_ctx();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "result=success\n").unwrap();

        FileCommandManager::process_output_file(
            &mut ctx,
            tmp.path().to_str().unwrap(),
        );

        assert_eq!(ctx.outputs.get("result"), Some(&"success".to_string()));
    }
}
