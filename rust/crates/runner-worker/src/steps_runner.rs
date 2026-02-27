// StepsRunner mapping `StepsRunner.cs`.
// Drains the job_steps queue then post_job_steps stack, evaluating conditions,
// enforcing timeouts, and updating the overall job result.
// Also reports step status and uploads logs to the Results Service.

use anyhow::Result;
use chrono::Utc;
use runner_common::util::task_result_util::{TaskResult, TaskResultUtil};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::action_command_manager::ActionCommandManager;
use crate::execution_context::ExecutionContext;
use crate::expressions::evaluate_condition;
use crate::file_command_manager::FileCommandManager;
use crate::results_client::{ResultsClient, StepConclusion, StepStatus, StepUpdate};

/// Executes all steps in a job, in order.
pub struct StepsRunner {
    /// Optional Results Service client for reporting step status and uploading logs.
    results_client: Option<Arc<ResultsClient>>,
}

impl StepsRunner {
    pub fn new() -> Self {
        Self {
            results_client: None,
        }
    }

    /// Set the Results Service client for step status reporting and log upload.
    pub fn with_results_client(mut self, client: Arc<ResultsClient>) -> Self {
        self.results_client = Some(client);
        self
    }

    /// Convert a TaskResult to Results Service StepConclusion.
    fn task_result_to_conclusion(result: TaskResult) -> StepConclusion {
        match result {
            TaskResult::Succeeded | TaskResult::SucceededWithIssues => StepConclusion::Success,
            TaskResult::Failed | TaskResult::Abandoned => StepConclusion::Failure,
            TaskResult::Canceled => StepConclusion::Cancelled,
            TaskResult::Skipped => StepConclusion::Skipped,
        }
    }

    /// Report a step status update to the Results Service.
    async fn report_step_status(
        &self,
        step_id: &str,
        step_number: u32,
        display_name: &str,
        status: StepStatus,
        conclusion: StepConclusion,
        started_at: Option<&str>,
        completed_at: Option<&str>,
        change_order: u64,
    ) {
        if let Some(ref client) = self.results_client {
            let update = StepUpdate {
                external_id: step_id.to_string(),
                number: step_number,
                name: display_name.to_string(),
                status,
                started_at: started_at.map(|s| s.to_string()),
                completed_at: completed_at.map(|s| s.to_string()),
                conclusion,
            };

            let trace = SimpleTrace;
            if let Err(e) = client.update_workflow_steps(&[update], change_order, &trace).await {
                tracing::warn!("Failed to update step status: {:#}", e);
            }
        }
    }

    /// Upload step logs to the Results Service.
    async fn upload_logs(&self, step_id: &str, log_lines: &[String]) {
        if let Some(ref client) = self.results_client {
            let trace = SimpleTrace;
            if let Err(e) = client.upload_step_log(step_id, log_lines, &trace).await {
                tracing::warn!("Failed to upload step logs: {:#}", e);
            }
        }
    }

    /// Run all job steps and post-job steps.
    pub async fn run_async(&self, context: &mut ExecutionContext) -> Result<()> {
        let mut step_number: u32 = 0;
        let mut change_order: u64 = 0;

        // Phase 1: Drain the job_steps queue (main steps)
        while let Some(step) = context.job_steps.pop_front() {
            step_number += 1;
            let cancel = context.cancel_token();

            // Check cancellation
            if cancel.is_cancelled() {
                context.info(&format!(
                    "Skipping step '{}' due to job cancellation.",
                    step.display_name()
                ));
                continue;
            }

            // Evaluate the condition expression
            let should_run = self.evaluate_step_condition(context, step.as_ref());

            if !should_run {
                context.info(&format!(
                    "Skipping step '{}' (condition evaluated to false).",
                    step.display_name()
                ));
                // Record as skipped in steps context
                context.steps_context_mut().record_step(
                    step.id(),
                    TaskResult::Skipped,
                    TaskResult::Skipped,
                    std::collections::HashMap::new(),
                );

                // Report skipped status to Results Service
                change_order += 1;
                self.report_step_status(
                    step.id(),
                    step_number,
                    step.display_name(),
                    StepStatus::Completed,
                    StepConclusion::Skipped,
                    None,
                    Some(&Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
                    change_order,
                ).await;

                continue;
            }

            context.info(&format!("Starting step: {}", step.display_name()));

            // Report step as InProgress to Results Service
            let started_at = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            change_order += 1;
            self.report_step_status(
                step.id(),
                step_number,
                step.display_name(),
                StepStatus::InProgress,
                StepConclusion::Unknown,
                Some(&started_at),
                None,
                change_order,
            ).await;

            // Create step-level execution context
            let mut step_context = context.create_step_context(
                step.id().to_string(),
                step.display_name().to_string(),
            );

            // Initialize file commands for this step
            FileCommandManager::initialize_file_commands(&mut step_context);

            // Set up timeout
            let timeout_minutes = step.timeout_in_minutes();
            let timeout = if timeout_minutes > 0 {
                Duration::from_secs(timeout_minutes as u64 * 60)
            } else {
                Duration::from_secs(6 * 60 * 60) // default: 6 hours
            };

            // Run the step with timeout
            let step_result = self.run_step_with_timeout(
                &step,
                &mut step_context,
                timeout,
                cancel.clone(),
            ).await;

            // Process file commands after step execution
            FileCommandManager::process_file_commands(&mut step_context);

            // Determine step outcome
            let (outcome, conclusion) = match step_result {
                Ok(()) => {
                    let outcome = step_context.result().unwrap_or(TaskResult::Succeeded);
                    let conclusion = if step.continue_on_error() && outcome == TaskResult::Failed {
                        TaskResult::Succeeded
                    } else {
                        outcome
                    };
                    (outcome, conclusion)
                }
                Err(e) => {
                    step_context.error(&format!("Step failed: {:#}", e));
                    let outcome = TaskResult::Failed;
                    let conclusion = if step.continue_on_error() {
                        step_context.info("Step failed but continue-on-error is enabled.");
                        TaskResult::Succeeded
                    } else {
                        TaskResult::Failed
                    };
                    (outcome, conclusion)
                }
            };

            // Upload step logs to Results Service
            self.upload_logs(step.id(), step_context.log_lines()).await;

            // Report step as Completed to Results Service
            let completed_at = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            change_order += 1;
            self.report_step_status(
                step.id(),
                step_number,
                step.display_name(),
                StepStatus::Completed,
                Self::task_result_to_conclusion(conclusion),
                Some(&started_at),
                Some(&completed_at),
                change_order,
            ).await;

            // Record step outcome and outputs in steps context
            context.steps_context_mut().record_step(
                step.id(),
                outcome,
                conclusion,
                step_context.outputs.clone(),
            );

            // Merge outputs back to parent context
            for (key, value) in &step_context.outputs {
                context.outputs.insert(
                    format!("{}_{}", step.id(), key),
                    value.clone(),
                );
            }

            // Update overall job result
            let current = context.result();
            let merged = TaskResultUtil::merge_task_results(current, conclusion);
            context.set_result(merged);

            context.info(&format!(
                "Step '{}' completed with outcome={:?}, conclusion={:?}",
                step.display_name(),
                outcome,
                conclusion
            ));
        }

        // Phase 2: Execute post-job steps in reverse order (LIFO)
        let post_steps: Vec<_> = context.post_job_steps.drain(..).collect();
        for step in post_steps.into_iter().rev() {
            let cancel = context.cancel_token();

            context.info(&format!("Running post step: {}", step.display_name()));

            let mut step_context = context.create_step_context(
                step.id().to_string(),
                format!("Post {}", step.display_name()),
            );

            let timeout = Duration::from_secs(5 * 60); // 5 min default for post steps

            let step_result = self.run_step_with_timeout(
                &step,
                &mut step_context,
                timeout,
                cancel,
            ).await;

            if let Err(e) = step_result {
                step_context.warning(&format!("Post step '{}' failed: {:#}", step.display_name(), e));
            }
        }

        Ok(())
    }

    /// Run a single step with a timeout guard.
    async fn run_step_with_timeout(
        &self,
        step: &Box<dyn crate::execution_context::IStep>,
        context: &mut ExecutionContext,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<()> {
        let step_cancel = CancellationToken::new();

        tokio::select! {
            result = step.run_async(context) => {
                result
            }
            _ = tokio::time::sleep(timeout) => {
                context.error(&format!(
                    "The step '{}' has exceeded the maximum execution time of {} minutes.",
                    context.display_name(),
                    timeout.as_secs() / 60
                ));
                context.complete(TaskResult::Failed, Some("Step timed out"));
                anyhow::bail!("Step timed out after {:?}", timeout)
            }
            _ = cancel.cancelled() => {
                context.info("Step cancelled.");
                context.complete(TaskResult::Canceled, Some("Job was cancelled"));
                anyhow::bail!("Step cancelled")
            }
        }
    }

    /// Evaluate the `if:` condition expression for a step.
    fn evaluate_step_condition(&self, context: &ExecutionContext, step: &dyn crate::execution_context::IStep) -> bool {
        let condition = step.condition();

        // Empty condition defaults to "success()"
        if condition.is_empty() {
            return self.eval_status_function(context, "success");
        }

        // Evaluate known status functions
        match condition.trim() {
            "always()" => true,
            "success()" => self.eval_status_function(context, "success"),
            "failure()" => self.eval_status_function(context, "failure"),
            "cancelled()" => self.eval_status_function(context, "cancelled"),
            _ => {
                // For complex expressions, delegate to the expression evaluator
                let job_status = context.result().unwrap_or(TaskResult::Succeeded);
                let is_cancelled = context.cancel_token().is_cancelled();
                let expr_context = serde_json::to_value(context.build_expression_context()).unwrap_or_default();
                evaluate_condition(condition, job_status, is_cancelled, &expr_context)
            }
        }
    }

    /// Evaluate a status function against the current job state.
    fn eval_status_function(&self, context: &ExecutionContext, function: &str) -> bool {
        match function {
            "success" => {
                match context.result() {
                    None | Some(TaskResult::Succeeded) | Some(TaskResult::SucceededWithIssues) => true,
                    _ => false,
                }
            }
            "failure" => {
                matches!(context.result(), Some(TaskResult::Failed))
            }
            "cancelled" => {
                matches!(context.result(), Some(TaskResult::Canceled))
            }
            _ => true,
        }
    }
}

/// Simple trace writer for Results Service logging.
struct SimpleTrace;

impl runner_sdk::TraceWriter for SimpleTrace {
    fn info(&self, message: &str) {
        tracing::info!(target: "results", "{}", message);
    }
    fn verbose(&self, message: &str) {
        tracing::debug!(target: "results", "{}", message);
    }
    fn error(&self, message: &str) {
        tracing::error!(target: "results", "{}", message);
    }
}

/// Convert a TaskResult to the outcome string used in steps context.
fn task_result_to_outcome_string(result: TaskResult) -> String {
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
    fn test_task_result_to_outcome_string() {
        assert_eq!(task_result_to_outcome_string(TaskResult::Succeeded), "success");
        assert_eq!(task_result_to_outcome_string(TaskResult::Failed), "failure");
        assert_eq!(task_result_to_outcome_string(TaskResult::Canceled), "cancelled");
        assert_eq!(task_result_to_outcome_string(TaskResult::Skipped), "skipped");
    }
}
