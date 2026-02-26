// OutputManager mapping `OutputManager.cs`.
// Processes stdout/stderr lines from step execution.
// Passes lines through ActionCommandManager for :: commands,
// applies issue matchers, and strips internal markers.

use crate::action_command_manager::ActionCommandManager;
use crate::execution_context::ExecutionContext;
use crate::issue_matcher::IssueMatcher;

/// Processes output lines from step execution.
pub struct OutputManager<'a> {
    /// The execution context for the current step.
    context: &'a mut ExecutionContext,

    /// Action command processor.
    command_manager: ActionCommandManager,

    /// Active issue matchers.
    issue_matchers: Vec<IssueMatcher>,
}

impl<'a> OutputManager<'a> {
    /// Create a new `OutputManager` for the given execution context.
    pub fn new(context: &'a mut ExecutionContext) -> Self {
        Self {
            context,
            command_manager: ActionCommandManager::new(),
            issue_matchers: Vec::new(),
        }
    }

    /// Add an issue matcher to this output manager.
    pub fn add_matcher(&mut self, matcher: IssueMatcher) {
        self.issue_matchers.push(matcher);
    }

    /// Remove an issue matcher by owner name.
    pub fn remove_matcher(&mut self, owner: &str) {
        self.issue_matchers.retain(|m| m.owner() != owner);
    }

    /// Process a single line of stdout output.
    pub fn on_stdout_data(&mut self, line: &str) {
        self.process_line(line, false);
    }

    /// Process a single line of stderr output.
    pub fn on_stderr_data(&mut self, line: &str) {
        self.process_line(line, true);
    }

    /// Process a single output line.
    fn process_line(&mut self, line: &str, _is_stderr: bool) {
        // Strip runner-internal markers
        let line = strip_internal_markers(line);

        // Check if it's a workflow command
        if self.command_manager.try_process_command(self.context, &line) {
            return;
        }

        // Try issue matchers
        for matcher in &self.issue_matchers {
            if let Some(issue) = matcher.try_match(&line) {
                match issue.severity.as_str() {
                    "error" => {
                        let msg = format_issue_message(&issue);
                        self.context.error(&msg);
                    }
                    "warning" => {
                        let msg = format_issue_message(&issue);
                        self.context.warning(&msg);
                    }
                    "notice" => {
                        let msg = format_issue_message(&issue);
                        self.context.info(&msg);
                    }
                    _ => {}
                }
                return;
            }
        }

        // Regular output - just write it
        self.context.write(&line);
    }
}

/// Strip runner-internal markers from a line.
fn strip_internal_markers(line: &str) -> String {
    // Remove internal telemetry markers
    if line.starts_with(runner_common::constants::PLUGIN_TRACE_PREFIX) {
        return String::new();
    }
    line.to_string()
}

/// Format an issue match into an annotation message.
fn format_issue_message(issue: &crate::issue_matcher::IssueMatch) -> String {
    let mut parts = Vec::new();

    if let Some(ref file) = issue.file {
        parts.push(format!("file={}", file));
    }
    if let Some(line) = issue.line {
        parts.push(format!("line={}", line));
    }
    if let Some(col) = issue.column {
        parts.push(format!("col={}", col));
    }

    if parts.is_empty() {
        issue.message.clone()
    } else {
        format!("{}: {}", parts.join(","), issue.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_internal_markers() {
        let line = "##[plugin.trace]some internal info";
        assert_eq!(strip_internal_markers(line), "");

        let line = "regular output";
        assert_eq!(strip_internal_markers(line), "regular output");
    }
}
