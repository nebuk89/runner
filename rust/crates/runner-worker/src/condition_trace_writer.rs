// ConditionTraceWriter mapping condition tracing in `ConditionFunctions.cs`.
// Logs details about condition evaluation for debugging.

use std::fmt::Write;

use runner_common::util::task_result_util::TaskResult;

/// Traces condition evaluation for debugging purposes.
///
/// When debug mode is enabled, this captures the evaluation steps
/// so the user can understand why a step was run or skipped.
pub struct ConditionTraceWriter {
    /// Whether tracing is enabled.
    enabled: bool,

    /// Accumulated trace messages.
    traces: Vec<String>,
}

impl ConditionTraceWriter {
    /// Create a new trace writer.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            traces: Vec::new(),
        }
    }

    /// Check if tracing is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Trace the start of condition evaluation.
    pub fn trace_condition_start(&mut self, condition: &str, step_name: &str) {
        if !self.enabled {
            return;
        }
        self.traces.push(format!(
            "Evaluating condition for step '{}': {}",
            step_name, condition
        ));
    }

    /// Trace a status function evaluation.
    pub fn trace_status_function(
        &mut self,
        function_name: &str,
        job_status: TaskResult,
        is_cancelled: bool,
        result: bool,
    ) {
        if !self.enabled {
            return;
        }
        self.traces.push(format!(
            "  {}() => {} (job_status={:?}, is_cancelled={})",
            function_name, result, job_status, is_cancelled
        ));
    }

    /// Trace a comparison evaluation.
    pub fn trace_comparison(
        &mut self,
        left: &str,
        operator: &str,
        right: &str,
        left_value: &str,
        right_value: &str,
        result: bool,
    ) {
        if !self.enabled {
            return;
        }
        self.traces.push(format!(
            "  {} {} {} => '{}' {} '{}' => {}",
            left, operator, right, left_value, operator, right_value, result
        ));
    }

    /// Trace a function call evaluation.
    pub fn trace_function_call(
        &mut self,
        function_name: &str,
        args: &[&str],
        result: bool,
    ) {
        if !self.enabled {
            return;
        }
        let args_str = args.join(", ");
        self.traces.push(format!(
            "  {}({}) => {}",
            function_name, args_str, result
        ));
    }

    /// Trace a context value resolution.
    pub fn trace_value_resolution(
        &mut self,
        expression: &str,
        resolved_value: &str,
    ) {
        if !self.enabled {
            return;
        }
        self.traces.push(format!(
            "  {} => '{}'",
            expression, resolved_value
        ));
    }

    /// Trace the final result of condition evaluation.
    pub fn trace_condition_result(&mut self, step_name: &str, result: bool) {
        if !self.enabled {
            return;
        }
        let action = if result { "will execute" } else { "will be skipped" };
        self.traces.push(format!(
            "Step '{}' {} (condition evaluated to {})",
            step_name, action, result
        ));
    }

    /// Get all accumulated trace messages.
    pub fn get_traces(&self) -> &[String] {
        &self.traces
    }

    /// Consume the writer and return all traces as a single formatted string.
    pub fn into_trace_string(self) -> String {
        self.traces.join("\n")
    }

    /// Clear all accumulated traces.
    pub fn clear(&mut self) {
        self.traces.clear();
    }

    /// Format a condition evaluation summary.
    pub fn format_evaluation_summary(
        condition: &str,
        step_name: &str,
        job_status: TaskResult,
        is_cancelled: bool,
        result: bool,
    ) -> String {
        let mut summary = String::new();
        let _ = writeln!(&mut summary, "Condition evaluation for '{}':", step_name);
        let _ = writeln!(&mut summary, "  Expression: {}", condition);
        let _ = writeln!(&mut summary, "  Job status: {:?}", job_status);
        let _ = writeln!(&mut summary, "  Cancelled: {}", is_cancelled);
        let _ = writeln!(&mut summary, "  Result: {}", result);
        summary
    }
}

impl Default for ConditionTraceWriter {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_tracing() {
        let mut writer = ConditionTraceWriter::new(false);
        writer.trace_condition_start("success()", "step1");
        assert!(writer.get_traces().is_empty());
    }

    #[test]
    fn test_enabled_tracing() {
        let mut writer = ConditionTraceWriter::new(true);
        writer.trace_condition_start("success()", "step1");
        assert_eq!(writer.get_traces().len(), 1);
        assert!(writer.get_traces()[0].contains("step1"));
        assert!(writer.get_traces()[0].contains("success()"));
    }

    #[test]
    fn test_status_function_trace() {
        let mut writer = ConditionTraceWriter::new(true);
        writer.trace_status_function(
            "success",
            TaskResult::Succeeded,
            false,
            true,
        );
        assert!(writer.get_traces()[0].contains("success()"));
        assert!(writer.get_traces()[0].contains("true"));
    }

    #[test]
    fn test_comparison_trace() {
        let mut writer = ConditionTraceWriter::new(true);
        writer.trace_comparison(
            "github.event_name",
            "==",
            "'push'",
            "push",
            "push",
            true,
        );
        let trace = &writer.get_traces()[0];
        assert!(trace.contains("github.event_name"));
        assert!(trace.contains("=="));
        assert!(trace.contains("push"));
    }

    #[test]
    fn test_into_trace_string() {
        let mut writer = ConditionTraceWriter::new(true);
        writer.trace_condition_start("always()", "deploy");
        writer.trace_condition_result("deploy", true);
        let result = writer.into_trace_string();
        assert!(result.contains("deploy"));
        assert!(result.contains("will execute"));
    }

    #[test]
    fn test_clear() {
        let mut writer = ConditionTraceWriter::new(true);
        writer.trace_condition_start("test", "step");
        assert!(!writer.get_traces().is_empty());
        writer.clear();
        assert!(writer.get_traces().is_empty());
    }

    #[test]
    fn test_format_evaluation_summary() {
        let summary = ConditionTraceWriter::format_evaluation_summary(
            "success()",
            "build",
            TaskResult::Succeeded,
            false,
            true,
        );
        assert!(summary.contains("build"));
        assert!(summary.contains("success()"));
        assert!(summary.contains("true"));
    }
}
