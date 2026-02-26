// ActionCommandManager mapping `ActionCommandManager.cs`.
// Processes workflow commands (::command key=val::data) embedded in step output lines.

use std::collections::{HashMap, HashSet};

use runner_common::action_command::ActionCommand;

use crate::execution_context::ExecutionContext;

/// All recognized workflow command names.
const WORKFLOW_COMMANDS: &[&str] = &[
    "set-output",
    "set-env",
    "add-path",
    "add-mask",
    "add-matcher",
    "remove-matcher",
    "warning",
    "error",
    "notice",
    "debug",
    "group",
    "endgroup",
    "echo",
    "save-state",
    "stop-commands",
];

/// Manager that processes workflow commands from step output lines.
pub struct ActionCommandManager {
    /// The set of registered command names.
    registered_commands: HashSet<String>,

    /// Stop-commands token: when set, all commands except the resume token are ignored.
    stop_token: Option<String>,

    /// Whether command echoing is enabled.
    echo_on_action_command: bool,
}

impl ActionCommandManager {
    /// Create a new `ActionCommandManager` with all standard commands registered.
    pub fn new() -> Self {
        let registered_commands: HashSet<String> = WORKFLOW_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Self {
            registered_commands,
            stop_token: None,
            echo_on_action_command: false,
        }
    }

    /// Try to process a workflow command from a line of output.
    ///
    /// Returns `true` if the line was a recognized command and was processed.
    pub fn try_process_command(
        &mut self,
        context: &mut ExecutionContext,
        line: &str,
    ) -> bool {
        // Skip empty lines
        if line.is_empty() {
            return false;
        }

        // Check for stop-commands token resume
        if let Some(ref token) = self.stop_token.clone() {
            // Look for the resume token: `::TOKEN::`
            if let Some(cmd) = ActionCommand::try_parse_v2(line, &self.registered_commands_with_token(token)) {
                if cmd.command == *token {
                    self.stop_token = None;
                    context.debug("Resuming workflow commands.");
                    return true;
                }
            }
            // While stopped, don't process any commands
            return false;
        }

        // Try to parse as a v2 command
        let cmd = match ActionCommand::try_parse_v2(line, &self.registered_commands) {
            Some(cmd) => cmd,
            None => return false,
        };

        // Echo the command if echoing is enabled
        if self.echo_on_action_command {
            context.write_command(line);
        }

        // Dispatch to the appropriate handler
        self.dispatch_command(context, &cmd);
        true
    }

    /// Helper to build a registered commands set that includes the stop token.
    fn registered_commands_with_token(&self, token: &str) -> HashSet<String> {
        let mut set = HashSet::new();
        set.insert(token.to_string());
        set
    }

    /// Dispatch a parsed command to its handler.
    fn dispatch_command(&mut self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        match cmd.command.as_str() {
            "set-output" => self.handle_set_output(context, cmd),
            "set-env" => self.handle_set_env(context, cmd),
            "add-path" => self.handle_add_path(context, cmd),
            "add-mask" => self.handle_add_mask(context, cmd),
            "add-matcher" => self.handle_add_matcher(context, cmd),
            "remove-matcher" => self.handle_remove_matcher(context, cmd),
            "warning" => self.handle_warning(context, cmd),
            "error" => self.handle_error(context, cmd),
            "notice" => self.handle_notice(context, cmd),
            "debug" => self.handle_debug(context, cmd),
            "group" => self.handle_group(context, cmd),
            "endgroup" => self.handle_endgroup(context, cmd),
            "echo" => self.handle_echo(context, cmd),
            "save-state" => self.handle_save_state(context, cmd),
            "stop-commands" => self.handle_stop_commands(context, cmd),
            unknown => {
                context.warning(&format!("Unknown workflow command: {}", unknown));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Command handlers
    // -----------------------------------------------------------------------

    fn handle_set_output(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let name = match cmd.properties.get("name") {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                context.warning("'set-output' command requires a 'name' property.");
                return;
            }
        };

        let value = cmd.data.clone();
        context.debug(&format!("Set output {}={}", name, value));
        context.outputs.insert(name, value);
    }

    fn handle_set_env(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let name = match cmd.properties.get("name") {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                context.warning("'set-env' command requires a 'name' property.");
                return;
            }
        };

        let value = cmd.data.clone();

        // Security: disallow setting certain critical environment variables
        let blocked = ["github_token", "github_auth", "actions_runtime_token"];
        if blocked.iter().any(|b| name.eq_ignore_ascii_case(b)) {
            context.warning(&format!(
                "Setting environment variable '{}' is not allowed.",
                name
            ));
            return;
        }

        context.debug(&format!("Setting env {}={}", name, value));
        context.global_mut().environment_variables.insert(name, value);
    }

    fn handle_add_path(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let path = cmd.data.trim().to_string();
        if path.is_empty() {
            context.warning("'add-path' command requires a non-empty path.");
            return;
        }
        context.debug(&format!("Prepending PATH: {}", path));
        context.global_mut().prepend_path.push(path);
    }

    fn handle_add_mask(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let value = cmd.data.trim().to_string();
        if value.is_empty() {
            context.debug("'add-mask' command received empty value, ignoring.");
            return;
        }
        context.debug("Adding mask for secret value.");
        context.secret_masker().add_value(&value);
    }

    fn handle_add_matcher(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let config_path = cmd.data.trim().to_string();
        if config_path.is_empty() {
            context.warning("'add-matcher' command requires a config file path.");
            return;
        }
        context.debug(&format!("Adding problem matcher from: {}", config_path));
        // Problem matcher loading is handled by the issue_matcher module
    }

    fn handle_remove_matcher(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let owner = match cmd.properties.get("owner") {
            Some(owner) if !owner.is_empty() => owner.clone(),
            _ => {
                context.warning("'remove-matcher' command requires an 'owner' property.");
                return;
            }
        };
        context.debug(&format!("Removing problem matcher: {}", owner));
    }

    fn handle_warning(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let message = self.format_annotation_message(cmd);
        context.warning(&message);
    }

    fn handle_error(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let message = self.format_annotation_message(cmd);
        context.error(&message);
    }

    fn handle_notice(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let message = self.format_annotation_message(cmd);
        context.info(&format!("Notice: {}", message));
    }

    fn handle_debug(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        context.debug(&cmd.data);
    }

    fn handle_group(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        context.section(&cmd.data);
    }

    fn handle_endgroup(&self, context: &mut ExecutionContext, _cmd: &ActionCommand) {
        context.end_section();
    }

    fn handle_echo(&mut self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        match cmd.data.trim().to_lowercase().as_str() {
            "on" => {
                context.debug("Enabling command echoing.");
                self.echo_on_action_command = true;
            }
            "off" => {
                context.debug("Disabling command echoing.");
                self.echo_on_action_command = false;
            }
            other => {
                context.warning(&format!(
                    "'echo' command expects 'on' or 'off', got '{}'",
                    other
                ));
            }
        }
    }

    fn handle_save_state(&self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let name = match cmd.properties.get("name") {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                context.warning("'save-state' command requires a 'name' property.");
                return;
            }
        };
        let value = cmd.data.clone();
        context.debug(&format!("Saving state: {}={}", name, value));
        // State is stored in outputs with a special prefix
        context.outputs.insert(format!("STATE_{}", name), value);
    }

    fn handle_stop_commands(&mut self, context: &mut ExecutionContext, cmd: &ActionCommand) {
        let token = cmd.data.trim().to_string();
        if token.is_empty() || token == "pause-logging" {
            context.warning(
                "The stop-commands token must not be empty or 'pause-logging'.",
            );
            return;
        }
        context.debug(&format!("Stopping workflow commands until token: {}", token));
        self.stop_token = Some(token);
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Format an annotation message with optional file/line/col properties.
    fn format_annotation_message(&self, cmd: &ActionCommand) -> String {
        let mut parts = Vec::new();

        if let Some(file) = cmd.properties.get("file") {
            if !file.is_empty() {
                parts.push(format!("file={}", file));
            }
        }
        if let Some(line) = cmd.properties.get("line") {
            if !line.is_empty() {
                parts.push(format!("line={}", line));
            }
        }
        if let Some(col) = cmd.properties.get("col") {
            if !col.is_empty() {
                parts.push(format!("col={}", col));
            }
        }
        if let Some(title) = cmd.properties.get("title") {
            if !title.is_empty() {
                parts.push(format!("title={}", title));
            }
        }

        if parts.is_empty() {
            cmd.data.clone()
        } else {
            format!("{}: {}", parts.join(","), cmd.data)
        }
    }
}

impl Default for ActionCommandManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_context::{ExecutionContext, Global};
    use crate::feature_manager::FeatureManager;
    use crate::variables::Variables;
    use runner_common::host_context::HostContext;
    use tokio_util::sync::CancellationToken;

    fn make_test_context() -> ExecutionContext {
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
            temp_directory: "/tmp/t".to_string(),
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
    fn test_set_output_command() {
        let mut mgr = ActionCommandManager::new();
        let mut ctx = make_test_context();
        let processed = mgr.try_process_command(&mut ctx, "::set-output name=result::hello");
        assert!(processed);
        assert_eq!(ctx.outputs.get("result"), Some(&"hello".to_string()));
    }

    #[test]
    fn test_debug_command() {
        let mut mgr = ActionCommandManager::new();
        let mut ctx = make_test_context();
        let processed = mgr.try_process_command(&mut ctx, "::debug::some debug info");
        assert!(processed);
    }

    #[test]
    fn test_stop_and_resume_commands() {
        let mut mgr = ActionCommandManager::new();
        let mut ctx = make_test_context();

        // Stop commands with token "mytoken"
        mgr.try_process_command(&mut ctx, "::stop-commands::mytoken");
        assert!(mgr.stop_token.is_some());

        // Commands should be ignored while stopped
        let processed = mgr.try_process_command(&mut ctx, "::debug::should be ignored");
        assert!(!processed);

        // Resume with the token
        mgr.try_process_command(&mut ctx, "::mytoken::");
        assert!(mgr.stop_token.is_none());
    }

    #[test]
    fn test_non_command_line() {
        let mut mgr = ActionCommandManager::new();
        let mut ctx = make_test_context();
        let processed = mgr.try_process_command(&mut ctx, "just a regular log line");
        assert!(!processed);
    }

    #[test]
    fn test_echo_on_off() {
        let mut mgr = ActionCommandManager::new();
        let mut ctx = make_test_context();
        mgr.try_process_command(&mut ctx, "::echo::on");
        assert!(mgr.echo_on_action_command);
        mgr.try_process_command(&mut ctx, "::echo::off");
        assert!(!mgr.echo_on_action_command);
    }
}
