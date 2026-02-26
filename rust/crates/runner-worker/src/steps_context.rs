// StepsContext mapping `StepsContext.cs`.
// Tracks step outcomes and outputs for the `steps.*` expression context.

use std::collections::HashMap;

use runner_common::util::task_result_util::TaskResult;

/// Recorded result for a single step.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StepResult {
    /// Outcome before `continue-on-error` adjustment.
    pub outcome: String,

    /// Conclusion after `continue-on-error` adjustment.
    pub conclusion: String,

    /// Step outputs (key → value).
    pub outputs: HashMap<String, String>,
}

/// Tracks the results and outputs of all executed steps.
///
/// Used to populate `steps.<id>.outcome`, `steps.<id>.conclusion`,
/// and `steps.<id>.outputs.<name>` in expression evaluation.
#[derive(Debug, Clone, Default)]
pub struct StepsContext {
    /// Map of step id → step result.
    results: HashMap<String, StepResult>,
}

impl StepsContext {
    /// Create a new, empty `StepsContext`.
    pub fn new() -> Self {
        Self {
            results: HashMap::new(),
        }
    }

    /// Record the result of a step.
    ///
    /// - `outcome` is the raw TaskResult (before continue-on-error).
    /// - `conclusion` is the final TaskResult (after continue-on-error).
    /// - `outputs` are the step's output key-value pairs.
    pub fn record_step(
        &mut self,
        step_id: &str,
        outcome: TaskResult,
        conclusion: TaskResult,
        outputs: HashMap<String, String>,
    ) {
        self.results.insert(
            step_id.to_string(),
            StepResult {
                outcome: task_result_to_string(outcome),
                conclusion: task_result_to_string(conclusion),
                outputs,
            },
        );
    }

    /// Check if a step has been recorded.
    pub fn has_step(&self, step_id: &str) -> bool {
        self.results.contains_key(step_id)
    }

    /// Get the outcome of a step.
    pub fn get_outcome(&self, step_id: &str) -> Option<&str> {
        self.results.get(step_id).map(|r| r.outcome.as_str())
    }

    /// Get the conclusion of a step.
    pub fn get_conclusion(&self, step_id: &str) -> Option<&str> {
        self.results.get(step_id).map(|r| r.conclusion.as_str())
    }

    /// Get a specific output from a step.
    pub fn get_output(&self, step_id: &str, output_name: &str) -> Option<&str> {
        self.results
            .get(step_id)
            .and_then(|r| r.outputs.get(output_name))
            .map(|s| s.as_str())
    }

    /// Get all outputs for a step.
    pub fn get_outputs(&self, step_id: &str) -> Option<&HashMap<String, String>> {
        self.results.get(step_id).map(|r| &r.outputs)
    }

    /// Get all recorded steps.
    pub fn steps(&self) -> &HashMap<String, StepResult> {
        &self.results
    }

    /// Convert to a serde_json::Value for expression evaluation.
    pub fn to_value(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (id, result) in &self.results {
            let mut step_map = serde_json::Map::new();
            step_map.insert(
                "outcome".to_string(),
                serde_json::Value::String(result.outcome.clone()),
            );
            step_map.insert(
                "conclusion".to_string(),
                serde_json::Value::String(result.conclusion.clone()),
            );

            let mut outputs_map = serde_json::Map::new();
            for (k, v) in &result.outputs {
                outputs_map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            step_map.insert("outputs".to_string(), serde_json::Value::Object(outputs_map));

            map.insert(id.clone(), serde_json::Value::Object(step_map));
        }
        serde_json::Value::Object(map)
    }
}

/// Convert a TaskResult enum to the GitHub Actions status string.
fn task_result_to_string(result: TaskResult) -> String {
    match result {
        TaskResult::Succeeded | TaskResult::SucceededWithIssues => "success".to_string(),
        TaskResult::Failed | TaskResult::Abandoned => "failure".to_string(),
        TaskResult::Canceled => "cancelled".to_string(),
        TaskResult::Skipped => "skipped".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_empty() {
        let ctx = StepsContext::new();
        assert_eq!(ctx.steps().len(), 0);
    }

    #[test]
    fn test_record_step() {
        let mut ctx = StepsContext::new();
        let mut outputs = HashMap::new();
        outputs.insert("result".to_string(), "42".to_string());

        ctx.record_step(
            "step1",
            TaskResult::Succeeded,
            TaskResult::Succeeded,
            outputs.clone(),
        );

        assert!(ctx.has_step("step1"));
        assert_eq!(ctx.get_outcome("step1"), Some("success"));
        assert_eq!(ctx.get_conclusion("step1"), Some("success"));
        assert_eq!(ctx.get_output("step1", "result"), Some("42"));
    }

    #[test]
    fn test_continue_on_error_different_outcome_conclusion() {
        let mut ctx = StepsContext::new();

        ctx.record_step(
            "failing_step",
            TaskResult::Failed,      // outcome
            TaskResult::Succeeded,   // conclusion (continue-on-error: true)
            HashMap::new(),
        );

        assert_eq!(ctx.get_outcome("failing_step"), Some("failure"));
        assert_eq!(ctx.get_conclusion("failing_step"), Some("success"));
    }

    #[test]
    fn test_to_value() {
        let mut ctx = StepsContext::new();
        let mut outputs = HashMap::new();
        outputs.insert("name".to_string(), "hello".to_string());
        ctx.record_step("step1", TaskResult::Succeeded, TaskResult::Succeeded, outputs);

        let val = ctx.to_value();
        let step1 = val.get("step1").unwrap();
        assert_eq!(step1.get("outcome").unwrap().as_str(), Some("success"));
        assert_eq!(step1.get("conclusion").unwrap().as_str(), Some("success"));
        assert_eq!(
            step1
                .get("outputs")
                .unwrap()
                .get("name")
                .unwrap()
                .as_str(),
            Some("hello")
        );
    }

    #[test]
    fn test_missing_step() {
        let ctx = StepsContext::new();
        assert!(!ctx.has_step("nonexistent"));
        assert_eq!(ctx.get_outcome("nonexistent"), None);
        assert_eq!(ctx.get_conclusion("nonexistent"), None);
        assert_eq!(ctx.get_output("nonexistent", "key"), None);
    }
}
