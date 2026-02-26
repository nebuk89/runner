// runner-common: Shared services and infrastructure for the GitHub Actions Runner.
// This crate maps the C# `Runner.Common` project and depends on `runner-sdk`.

pub mod action_command;
pub mod action_result;
pub mod config_store;
pub mod constants;
pub mod credential_data;
pub mod exceptions;
pub mod host_context;
pub mod http_client_factory;
pub mod job_notification;
pub mod logging;
pub mod process_channel;
pub mod process_invoker;
pub mod runner_service;
pub mod secret_masker;
pub mod terminal;
pub mod tracing;
pub mod util;

// ---------------------------------------------------------------------------
// Re-exports for convenient access
// ---------------------------------------------------------------------------

pub use action_command::ActionCommand;
pub use action_result::ActionResult;
pub use config_store::{ConfigurationStore, RunnerSettings};
pub use constants::{
    Architecture, OsPlatform, WellKnownConfigFile, WellKnownDirectory, CURRENT_ARCHITECTURE,
    CURRENT_PLATFORM,
};
pub use credential_data::CredentialData;
pub use exceptions::NonRetryableException;
pub use host_context::HostContext;
pub use http_client_factory::HttpClientFactory;
pub use job_notification::JobNotification;
pub use logging::PagingLogger;
pub use process_channel::{MessageType, ProcessChannel, WorkerMessage};
pub use process_invoker::ProcessInvokerService;
pub use runner_service::{RunnerService, ServiceLocator, ShutdownReason, StartupType};
pub use secret_masker::SecretMasker;
pub use terminal::Terminal;
pub use tracing::{TraceEventType, TraceManager, TraceSetting, Tracing};
pub use util::encoding_util::EncodingUtil;
pub use util::node_util::NodeUtil;
pub use util::task_result_util::{TaskResult, TaskResultUtil};
pub use util::var_util::VarUtil;
