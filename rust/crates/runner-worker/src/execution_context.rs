// ExecutionContext mapping `ExecutionContext.cs`.
// THE central mutable state for a running job. Holds variables, endpoints,
// step queues, logging methods, result tracking, and expression context building.

use parking_lot::RwLock;
use runner_common::host_context::HostContext;
use runner_common::secret_masker::SecretMasker;
use runner_common::util::task_result_util::TaskResult;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::feature_manager::FeatureManager;
use crate::github_context::GitHubContext;
use crate::runner_context::RunnerContext;
use crate::steps_context::StepsContext;
use crate::variables::Variables;
use crate::worker::ServiceEndpoint;

// ---------------------------------------------------------------------------
// IStep trait
// ---------------------------------------------------------------------------

/// A step that can be executed by the StepsRunner.
pub trait IStep: Send + Sync {
    /// Unique identifier for this step.
    fn id(&self) -> &str;

    /// Human-friendly display name.
    fn display_name(&self) -> &str;

    /// Condition expression (e.g. "success()", "always()").
    fn condition(&self) -> &str;

    /// Timeout for this step in minutes.
    fn timeout_in_minutes(&self) -> u32;

    /// Whether to continue on error.
    fn continue_on_error(&self) -> bool;

    /// The step type discriminator ("script", "action").
    fn step_type(&self) -> &str;

    /// Execute this step in the given execution context.
    fn run_async<'a>(
        &'a self,
        context: &'a mut ExecutionContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>>;

    /// Reference / action info, if applicable.
    fn reference_name(&self) -> Option<&str> {
        None
    }
}

// ---------------------------------------------------------------------------
// Global (shared state across all step contexts)
// ---------------------------------------------------------------------------

/// Shared mutable state for the entire job, accessible from all step contexts.
pub struct Global {
    /// Job-level variables.
    pub variables: Variables,

    /// Service endpoints from the job message.
    pub endpoints: Vec<ServiceEndpoint>,

    /// File table for file commands.
    pub file_table: Vec<String>,

    /// Job-level environment variables.
    pub environment_variables: HashMap<String, String>,

    /// Display name of the job.
    pub job_display_name: String,

    /// Job ID.
    pub job_id: String,

    /// Plan ID.
    pub plan_id: String,

    /// Timeline ID.
    pub timeline_id: String,

    /// Pipeline working directory.
    pub pipeline_directory: String,

    /// Workspace directory inside the pipeline directory.
    pub workspace_directory: String,

    /// Temp directory.
    pub temp_directory: String,

    /// Paths prepended to PATH by add-path commands.
    pub prepend_path: Vec<String>,

    /// Job container info (if running in a container).
    pub container_info: Option<crate::container::container_info::ContainerInfo>,

    /// Service containers.
    pub service_containers: Vec<crate::container::container_info::ContainerInfo>,

    /// Job-level telemetry entries.
    pub job_telemetry: Vec<String>,

    /// Environment URL for deployments.
    pub environment_url: Option<String>,

    /// Cancellation token for the job.
    pub cancel_token: CancellationToken,

    /// Feature flag manager.
    pub feature_manager: FeatureManager,

    /// Whether debug output is enabled.
    pub write_debug: bool,
}

// ---------------------------------------------------------------------------
// ExecutionContext
// ---------------------------------------------------------------------------

/// The central execution context for a job or step.
///
/// Holds all mutable state needed during execution: variables, step queues,
/// logging, results, and expression context data.
pub struct ExecutionContext {
    /// Reference to the host context for service lookups.
    host_context: Arc<HostContext>,

    /// Shared global state (wrapped in Arc<RwLock> for thread safety).
    global: Arc<RwLock<Global>>,

    /// Display name for this context (job name or step name).
    display_name: String,

    /// Current step ID, if this is a step-level context.
    current_step_id: Option<String>,

    /// The result of this context (step or job result).
    result: Option<TaskResult>,

    /// Result message (e.g. error message on failure).
    result_message: Option<String>,

    /// The ordered queue of main job steps to execute.
    pub job_steps: VecDeque<Box<dyn IStep>>,

    /// Post-job steps (executed in reverse after all main steps).
    pub post_job_steps: Vec<Box<dyn IStep>>,

    /// Outputs collected by this step context.
    pub outputs: HashMap<String, String>,

    /// Step-level environment variable overrides.
    pub step_environment: HashMap<String, String>,

    /// Runner context.
    runner_context: Option<RunnerContext>,

    /// GitHub context.
    github_context: Option<GitHubContext>,

    /// Steps context (accumulated step outcomes/outputs).
    steps_context: StepsContext,

    /// The secret masker reference for output sanitization.
    secret_masker: Arc<SecretMasker>,

    /// Accumulated log lines for this context.
    log_lines: Vec<String>,

    /// Whether this context has been completed.
    is_completed: bool,

    /// Embedded file command state.
    pub file_command_paths: HashMap<String, String>,

    /// Depth counter for child contexts (composite action recursion guard).
    depth: u32,
}

impl ExecutionContext {
    /// Create a new root execution context for a job.
    pub fn new_root(
        host_context: Arc<HostContext>,
        global: Global,
        display_name: String,
    ) -> Self {
        let secret_masker = Arc::clone(&host_context.secret_masker);
        Self {
            host_context,
            global: Arc::new(RwLock::new(global)),
            display_name,
            current_step_id: None,
            result: None,
            result_message: None,
            job_steps: VecDeque::new(),
            post_job_steps: Vec::new(),
            outputs: HashMap::new(),
            step_environment: HashMap::new(),
            runner_context: None,
            github_context: None,
            steps_context: StepsContext::new(),
            secret_masker,
            log_lines: Vec::new(),
            is_completed: false,
            file_command_paths: HashMap::new(),
            depth: 0,
        }
    }

    /// Create a child execution context for a step.
    pub fn create_step_context(&self, step_id: String, display_name: String) -> Self {
        Self {
            host_context: Arc::clone(&self.host_context),
            global: Arc::clone(&self.global),
            display_name,
            current_step_id: Some(step_id),
            result: None,
            result_message: None,
            job_steps: VecDeque::new(),
            post_job_steps: Vec::new(),
            outputs: HashMap::new(),
            step_environment: HashMap::new(),
            runner_context: self.runner_context.clone(),
            github_context: self.github_context.clone(),
            steps_context: self.steps_context.clone(),
            secret_masker: Arc::clone(&self.secret_masker),
            log_lines: Vec::new(),
            is_completed: false,
            file_command_paths: self.file_command_paths.clone(),
            depth: self.depth + 1,
        }
    }

    /// Create a child context for composite action steps (with depth tracking).
    pub fn create_child(&self, display_name: String) -> Self {
        Self {
            host_context: Arc::clone(&self.host_context),
            global: Arc::clone(&self.global),
            display_name,
            current_step_id: self.current_step_id.clone(),
            result: None,
            result_message: None,
            job_steps: VecDeque::new(),
            post_job_steps: Vec::new(),
            outputs: HashMap::new(),
            step_environment: self.step_environment.clone(),
            runner_context: self.runner_context.clone(),
            github_context: self.github_context.clone(),
            steps_context: StepsContext::new(),
            secret_masker: Arc::clone(&self.secret_masker),
            log_lines: Vec::new(),
            is_completed: false,
            file_command_paths: self.file_command_paths.clone(),
            depth: self.depth + 1,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Get the display name of this context.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Get the current step ID, if any.
    pub fn current_step_id(&self) -> Option<&str> {
        self.current_step_id.as_deref()
    }

    /// Get the host context.
    pub fn host_context(&self) -> &Arc<HostContext> {
        &self.host_context
    }

    /// Read access to the global shared state.
    pub fn global(&self) -> parking_lot::RwLockReadGuard<'_, Global> {
        self.global.read()
    }

    /// Write access to the global shared state.
    pub fn global_mut(&self) -> parking_lot::RwLockWriteGuard<'_, Global> {
        self.global.write()
    }

    /// Get the cancellation token for this job.
    pub fn cancel_token(&self) -> CancellationToken {
        self.global.read().cancel_token.clone()
    }

    /// Get the current result.
    pub fn result(&self) -> Option<TaskResult> {
        self.result
    }

    /// Get the current result message.
    pub fn result_message(&self) -> Option<&str> {
        self.result_message.as_deref()
    }

    /// Whether this context has been completed.
    pub fn is_completed(&self) -> bool {
        self.is_completed
    }

    /// Get the runner context.
    pub fn runner_context(&self) -> Option<&RunnerContext> {
        self.runner_context.as_ref()
    }

    /// Get the GitHub context.
    pub fn github_context(&self) -> Option<&GitHubContext> {
        self.github_context.as_ref()
    }

    /// Get a reference to the steps context.
    pub fn steps_context(&self) -> &StepsContext {
        &self.steps_context
    }

    /// Get a mutable reference to the steps context.
    pub fn steps_context_mut(&mut self) -> &mut StepsContext {
        &mut self.steps_context
    }

    /// Get the nesting depth.
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Get the secret masker.
    pub fn secret_masker(&self) -> &SecretMasker {
        &self.secret_masker
    }

    // -----------------------------------------------------------------------
    // Setters
    // -----------------------------------------------------------------------

    /// Set the runner context.
    pub fn set_runner_context(&mut self, ctx: RunnerContext) {
        self.runner_context = Some(ctx);
    }

    /// Set the GitHub context.
    pub fn set_github_context(&mut self, ctx: GitHubContext) {
        self.github_context = Some(ctx);
    }

    /// Set the result.
    pub fn set_result(&mut self, result: TaskResult) {
        self.result = Some(result);
    }

    // -----------------------------------------------------------------------
    // Logging
    // -----------------------------------------------------------------------

    /// Write a standard output line.
    pub fn write(&mut self, message: &str) {
        let masked = self.secret_masker.mask_secrets(message);
        self.log_lines.push(masked.clone());
        tracing::info!(target: "step", "[{}] {}", self.display_name, masked);
    }

    /// Write an informational message.
    pub fn info(&mut self, message: &str) {
        self.write(message);
    }

    /// Write a debug message (only if debug mode is enabled).
    pub fn debug(&mut self, message: &str) {
        if self.global.read().write_debug {
            let masked = self.secret_masker.mask_secrets(message);
            self.log_lines.push(format!("##[debug]{}", masked));
            tracing::debug!(target: "step", "[{}] {}", self.display_name, masked);
        }
    }

    /// Write a warning message.
    pub fn warning(&mut self, message: &str) {
        let masked = self.secret_masker.mask_secrets(message);
        self.log_lines.push(format!("##[warning]{}", masked));
        tracing::warn!(target: "step", "[{}] {}", self.display_name, masked);
    }

    /// Write an error message.
    pub fn error(&mut self, message: &str) {
        let masked = self.secret_masker.mask_secrets(message);
        self.log_lines.push(format!("##[error]{}", masked));
        tracing::error!(target: "step", "[{}] {}", self.display_name, masked);
    }

    /// Write a section / group header.
    pub fn section(&mut self, message: &str) {
        let masked = self.secret_masker.mask_secrets(message);
        self.log_lines.push(format!("##[group]{}", masked));
        tracing::info!(target: "step", "[{}] >> {}", self.display_name, masked);
    }

    /// Write an end-group marker.
    pub fn end_section(&mut self) {
        self.log_lines.push("##[endgroup]".to_string());
    }

    /// Write a ::command to the output stream.
    pub fn write_command(&mut self, command: &str) {
        self.log_lines.push(format!("##[command]{}", command));
        tracing::info!(target: "step", "[{}] [command]{}", self.display_name, command);
    }

    /// Get all log lines recorded in this context.
    pub fn log_lines(&self) -> &[String] {
        &self.log_lines
    }

    // -----------------------------------------------------------------------
    // Completion
    // -----------------------------------------------------------------------

    /// Mark this context as complete with the given result.
    pub fn complete(&mut self, result: TaskResult, message: Option<&str>) {
        if self.is_completed {
            tracing::warn!(
                "Attempted to complete already-completed context: {}",
                self.display_name
            );
            return;
        }

        self.result = Some(result);
        self.result_message = message.map(|s| s.to_string());
        self.is_completed = true;

        let level = match result {
            TaskResult::Succeeded | TaskResult::SucceededWithIssues => "info",
            TaskResult::Skipped => "info",
            _ => "error",
        };

        let msg = format!(
            "Finishing: {} (Result: {}{})",
            self.display_name,
            result,
            message.map(|m| format!(", Message: {}", m)).unwrap_or_default()
        );

        match level {
            "error" => tracing::error!(target: "step", "{}", msg),
            _ => tracing::info!(target: "step", "{}", msg),
        }
    }

    // -----------------------------------------------------------------------
    // Expression context building
    // -----------------------------------------------------------------------

    /// Build a map of expression context values for condition evaluation.
    /// This is used by the steps runner to evaluate `if:` conditions.
    pub fn build_expression_context(&self) -> HashMap<String, serde_json::Value> {
        let mut ctx = HashMap::new();

        // runner context
        if let Some(ref runner) = self.runner_context {
            ctx.insert("runner".to_string(), serde_json::to_value(runner).unwrap_or_default());
        }

        // github context
        if let Some(ref github) = self.github_context {
            ctx.insert("github".to_string(), serde_json::to_value(github).unwrap_or_default());
        }

        // steps context
        ctx.insert("steps".to_string(), self.steps_context.to_value());

        // env context
        let global = self.global.read();
        let mut env_map = global.environment_variables.clone();
        for (k, v) in &self.step_environment {
            env_map.insert(k.clone(), v.clone());
        }
        ctx.insert("env".to_string(), serde_json::to_value(&env_map).unwrap_or_default());

        // job context
        let mut job_map = HashMap::new();
        job_map.insert("status".to_string(), self.job_status_string());
        if let Some(ref container) = global.container_info {
            job_map.insert("container".to_string(), format!("{:?}", container));
        }
        ctx.insert("job".to_string(), serde_json::to_value(&job_map).unwrap_or_default());

        ctx
    }

    /// Get the current job status as a string for expression evaluation.
    fn job_status_string(&self) -> String {
        match self.result {
            Some(TaskResult::Failed) | Some(TaskResult::Abandoned) => "failure".to_string(),
            Some(TaskResult::Canceled) => "cancelled".to_string(),
            _ => "success".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_context() -> ExecutionContext {
        let host = HostContext::new("Test");
        let global = Global {
            variables: Variables::new(),
            endpoints: Vec::new(),
            file_table: Vec::new(),
            environment_variables: HashMap::new(),
            job_display_name: "test-job".to_string(),
            job_id: "job-1".to_string(),
            plan_id: "plan-1".to_string(),
            timeline_id: "tl-1".to_string(),
            pipeline_directory: "/tmp/pipeline".to_string(),
            workspace_directory: "/tmp/pipeline/workspace".to_string(),
            temp_directory: "/tmp/runner_temp".to_string(),
            prepend_path: Vec::new(),
            container_info: None,
            service_containers: Vec::new(),
            job_telemetry: Vec::new(),
            environment_url: None,
            cancel_token: CancellationToken::new(),
            feature_manager: FeatureManager::empty(),
            write_debug: true,
        };
        ExecutionContext::new_root(host, global, "test-job".to_string())
    }

    #[test]
    fn test_context_logging() {
        let mut ctx = make_test_context();
        ctx.write("Hello world");
        ctx.debug("Debug info");
        ctx.warning("A warning");
        ctx.error("An error");
        assert_eq!(ctx.log_lines().len(), 4);
    }

    #[test]
    fn test_context_completion() {
        let mut ctx = make_test_context();
        assert!(!ctx.is_completed());
        ctx.complete(TaskResult::Succeeded, None);
        assert!(ctx.is_completed());
        assert_eq!(ctx.result(), Some(TaskResult::Succeeded));
    }

    #[test]
    fn test_create_step_context() {
        let ctx = make_test_context();
        let step = ctx.create_step_context("step-1".to_string(), "Run tests".to_string());
        assert_eq!(step.current_step_id(), Some("step-1"));
        assert_eq!(step.display_name(), "Run tests");
        assert_eq!(step.depth(), 1);
    }

    #[test]
    fn test_double_complete_no_panic() {
        let mut ctx = make_test_context();
        ctx.complete(TaskResult::Succeeded, None);
        ctx.complete(TaskResult::Failed, Some("should be ignored"));
        assert_eq!(ctx.result(), Some(TaskResult::Succeeded));
    }
}
