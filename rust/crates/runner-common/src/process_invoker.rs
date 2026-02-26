// ProcessInvoker service wrapper mapping the Common-layer ProcessInvoker.
// This wraps the SDK-level `ProcessInvoker` with service-layer conveniences.

use crate::host_context::HostContext;
use crate::tracing::Tracing;

use anyhow::Result;
use runner_sdk::ProcessInvoker as SdkProcessInvoker;
use runner_sdk::TraceWriter;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// A service-level wrapper around the SDK `ProcessInvoker`.
///
/// Provides convenience methods with sensible defaults for common process
/// invocation patterns used throughout the runner.
pub struct ProcessInvokerService {
    context: Option<Arc<HostContext>>,
    trace: Option<Tracing>,
}

impl ProcessInvokerService {
    /// Create a new `ProcessInvokerService`.
    pub fn new() -> Self {
        Self {
            context: None,
            trace: None,
        }
    }

    /// Initialize with a host context.
    pub fn initialize(&mut self, context: Arc<HostContext>) {
        self.trace = Some(context.get_trace("ProcessInvoker"));
        self.context = Some(context);
    }

    /// Execute a process with full control over all parameters.
    pub async fn execute(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: Option<&HashMap<String, String>>,
        require_exit_code_zero: bool,
        kill_process_on_cancel: bool,
        cancellation_token: CancellationToken,
    ) -> Result<i32> {
        let trace = self.get_trace();
        let invoker = SdkProcessInvoker::new(Arc::new(trace.clone()) as Arc<dyn TraceWriter>);

        invoker
            .execute(
                working_directory,
                file_name,
                arguments,
                environment,
                require_exit_code_zero,
                kill_process_on_cancel,
                cancellation_token,
            )
            .await
    }

    /// Execute a process with default settings (require exit code zero, don't kill on cancel).
    pub async fn execute_simple(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        cancellation_token: CancellationToken,
    ) -> Result<i32> {
        self.execute(
            working_directory,
            file_name,
            arguments,
            None,
            true,
            false,
            cancellation_token,
        )
        .await
    }

    /// Execute a process with environment overrides.
    pub async fn execute_with_env(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: &HashMap<String, String>,
        cancellation_token: CancellationToken,
    ) -> Result<i32> {
        self.execute(
            working_directory,
            file_name,
            arguments,
            Some(environment),
            true,
            false,
            cancellation_token,
        )
        .await
    }

    /// Execute a process allowing non-zero exit codes (returns the exit code without error).
    pub async fn execute_allow_exit_codes(
        &self,
        working_directory: &str,
        file_name: &str,
        arguments: &str,
        environment: Option<&HashMap<String, String>>,
        cancellation_token: CancellationToken,
    ) -> Result<i32> {
        self.execute(
            working_directory,
            file_name,
            arguments,
            environment,
            false,
            false,
            cancellation_token,
        )
        .await
    }

    /// Get or create a trace writer for this service.
    fn get_trace(&self) -> Tracing {
        self.trace.clone().unwrap_or_else(|| {
            // Fallback: create a basic tracing instance
            use crate::secret_masker::SecretMasker;
            use crate::tracing::TraceSetting;
            Tracing::new(
                "ProcessInvoker",
                Arc::new(SecretMasker::new()),
                TraceSetting::default(),
            )
        })
    }
}

impl Default for ProcessInvokerService {
    fn default() -> Self {
        Self::new()
    }
}
