// Runner mapping `Runner.cs` — the main orchestrator.
// Dispatches CLI commands (configure/remove/run/warmup/check/help/version),
// runs the core message loop (create session, poll, dispatch jobs, handle updates).

use anyhow::{Context, Result};
use runner_common::config_store::{ConfigurationStore, RunnerSettings};
use runner_common::constants::{self, WellKnownDirectory};
use runner_common::host_context::HostContext;
use runner_common::runner_service::ShutdownReason;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::broker_message_listener::{BrokerMessageListener, BrokerMessageType};
use crate::checks;
use crate::command_settings::CommandSettings;
use crate::configuration::config_manager::ConfigManager;
use crate::error_throttler::ErrorThrottler;
use crate::job_dispatcher::{AgentJobRequestMessage, JobCancelMessage, JobDispatcher};
use crate::message_listener::{MessageListener, MessageType};
use crate::runner_config_updater::{RunnerConfigUpdater, RunnerRefreshConfigMessage};
use crate::self_updater::{AgentRefreshMessage, SelfUpdater};
use crate::self_updater_v2::{RunnerRefreshMessage, SelfUpdaterV2};

/// Delay between message poll iterations on empty response.
const MESSAGE_POLL_DELAY: Duration = Duration::from_secs(1);

/// Reference to a job from the V2 broker (minimal body in RunnerJobRequest).
#[derive(Debug, Clone, serde::Deserialize)]
struct RunnerJobRequestRef {
    runner_request_id: String,
    #[serde(default)]
    run_service_url: Option<String>,
    #[serde(default)]
    billing_owner_id: Option<String>,
    #[serde(default)]
    should_acknowledge: bool,
}

/// Payload for POST /acquirejob on the run service.
#[derive(Debug, serde::Serialize)]
struct AcquireJobRequest {
    #[serde(rename = "jobMessageId")]
    job_message_id: String,
    #[serde(rename = "runnerOS")]
    runner_os: String,
    #[serde(rename = "billingOwnerId")]
    billing_owner_id: String,
}

/// Grace period before force shutdown.
#[allow(dead_code)]
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// The main runner orchestrator.
///
/// Maps `Runner` in the C# runner (~1161 lines). This is the top-level
/// coordinator that:
/// - Parses CLI commands and dispatches to the appropriate handler
/// - Runs the main message loop (create session → poll → dispatch → repeat)
/// - Handles self-update, config refresh, graceful shutdown
pub struct Runner {
    context: Arc<HostContext>,
    trace: Tracing,
}

impl Runner {
    /// Create a new `Runner`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("Runner");
        Self { context, trace }
    }

    /// Parse CLI args and dispatch to the appropriate command handler.
    ///
    /// Returns the process exit code.
    pub async fn execute_command(&self) -> Result<i32> {
        let settings = CommandSettings::parse();

        self.trace.info(&format!(
            "Command: {:?}, Args: {:?}",
            settings.command(),
            settings.sanitized_args()
        ));

        // --version flag (can appear with any command or alone)
        if settings.is_version() {
            return self.print_version().await;
        }

        // --help flag
        if settings.is_help() {
            return self.print_help().await;
        }

        // --check flag (connectivity checks)
        if settings.is_check() {
            return self.run_checks(&settings).await;
        }

        // Dispatch by command
        match settings.command() {
            Some("configure") => self.configure(&settings).await,
            Some("remove") => self.remove(&settings).await,
            Some("warmup") => self.warmup().await,
            Some("run") | None => self.run_async(&settings).await,
            Some(cmd) => {
                self.trace
                    .error(&format!("Unknown command: '{}'", cmd));
                self.print_help().await
            }
        }
    }

    // -----------------------------------------------------------------------
    // Command handlers
    // -----------------------------------------------------------------------

    /// Handle the "configure" command.
    async fn configure(&self, settings: &CommandSettings) -> Result<i32> {
        self.trace.info("Executing 'configure' command");

        let config_manager = ConfigManager::new(self.context.clone());
        config_manager
            .configure_async(settings)
            .await
            .context("Configuration failed")?;

        self.trace.info("Configuration completed successfully");
        Ok(constants::return_code::SUCCESS)
    }

    /// Handle the "remove" command.
    async fn remove(&self, settings: &CommandSettings) -> Result<i32> {
        self.trace.info("Executing 'remove' command");

        let config_manager = ConfigManager::new(self.context.clone());
        config_manager
            .unconfigure_async(settings)
            .await
            .context("Remove failed")?;

        self.trace.info("Runner removed successfully");
        Ok(constants::return_code::SUCCESS)
    }

    /// Handle the "warmup" command.
    async fn warmup(&self) -> Result<i32> {
        self.trace.info("Executing 'warmup' command");

        // Warmup preloads the runner to reduce cold-start time
        // In the Rust version, this is mostly about ensuring the binary
        // and its dependencies are in the OS page cache.
        let config_store = ConfigurationStore::new(&self.context);

        if config_store.is_configured() {
            let _ = config_store.get_settings();
            self.trace.info("Warmup: settings loaded");
        }

        // Touch the externals directory to ensure it's cached
        let externals = self.context.get_directory(WellKnownDirectory::Externals);
        if externals.exists() {
            let _ = std::fs::read_dir(&externals);
        }

        self.trace.info("Warmup completed");
        Ok(constants::return_code::SUCCESS)
    }

    /// Print version information.
    async fn print_version(&self) -> Result<i32> {
        let version = runner_sdk::build_constants::RunnerPackage::VERSION;
        let commit = runner_sdk::build_constants::Source::COMMIT_HASH;
        println!("{}", version);
        self.trace.info(&format!("Version: {} ({})", version, commit));
        Ok(constants::return_code::SUCCESS)
    }

    /// Print help text.
    async fn print_help(&self) -> Result<i32> {
        println!("GitHub Actions Runner v{}", runner_sdk::build_constants::RunnerPackage::VERSION);
        println!();
        println!("Commands:");
        println!("  ./config.sh         Configure the runner");
        println!("  ./config.sh remove  Remove the runner");
        println!("  ./run.sh            Run the runner interactively");
        println!();
        println!("Options:");
        println!("  --help              Show this help message");
        println!("  --version           Show the runner version");
        println!("  --check             Run connectivity checks");
        println!("  --url <url>         URL of the repository/org/enterprise");
        println!("  --token <token>     Registration token");
        println!("  --name <name>       Name of the runner (default: hostname)");
        println!("  --work <dir>        Work directory (default: _work)");
        println!("  --labels <labels>   Extra labels (comma separated)");
        println!("  --runnergroup <grp> Runner group name");
        println!("  --replace           Replace existing runner with same name");
        println!("  --unattended        Run in unattended mode (no prompts)");
        println!("  --ephemeral         Configure as an ephemeral runner");
        println!("  --disableupdate     Disable automatic runner updates");
        println!("  --once              Run one job and then exit");
        println!("  --pat <pat>         Personal access token (for remove)");
        Ok(constants::return_code::SUCCESS)
    }

    /// Run connectivity checks.
    async fn run_checks(&self, settings: &CommandSettings) -> Result<i32> {
        self.trace.info("Running connectivity checks");
        let url = settings.get_url();
        let results = checks::run_all_checks(url.as_deref(), &self.trace).await;

        let output = checks::format_check_results(&results);
        println!("{}", output);

        let all_passed = results.iter().all(|r| r.passed);

        if all_passed {
            println!("\nAll checks passed.");
            Ok(constants::return_code::SUCCESS)
        } else {
            println!("\nSome checks failed.");
            Ok(constants::return_code::TERMINATED_ERROR)
        }
    }

    // -----------------------------------------------------------------------
    // Main message loop
    // -----------------------------------------------------------------------

    /// The core "run" loop: create session, poll messages, dispatch jobs.
    ///
    /// This is the heart of the runner. It:
    /// 1. Loads configuration
    /// 2. Sets up Ctrl-C / SIGTERM handlers for graceful shutdown
    /// 3. Creates a session (V1 or V2 depending on settings)
    /// 4. Polls for messages in a loop
    /// 5. Dispatches each message to the appropriate handler
    /// 6. Handles errors with exponential backoff
    async fn run_async(&self, settings: &CommandSettings) -> Result<i32> {
        self.trace.info("Executing 'run' command — entering message loop");

        // Verify the runner is configured
        let config_store = ConfigurationStore::new(&self.context);
        if !config_store.is_configured() {
            self.trace.error(
                "Runner is not configured. Run './config.sh' first.",
            );
            println!(
                "Runner is not configured. Run './config.sh' to configure."
            );
            return Ok(constants::return_code::TERMINATED_ERROR);
        }

        let runner_settings = config_store
            .get_settings()
            .context("Failed to load runner settings")?;

        // Set the work folder in the host context
        if !runner_settings.work_folder.is_empty() {
            self.context
                .set_work_folder(&runner_settings.work_folder);
        }

        // Determine run mode
        let is_run_once = settings.is_once() || runner_settings.is_ephemeral;
        let is_v2_flow = runner_settings.use_v2_flow;

        self.trace.info(&format!(
            "Runner settings: name={}, pool={}, ephemeral={}, v2_flow={}, run_once={}",
            runner_settings.agent_name,
            runner_settings.pool_name,
            runner_settings.is_ephemeral,
            is_v2_flow,
            is_run_once,
        ));

        // Set up Ctrl+C / SIGTERM handler
        let shutdown_token = self.context.runner_shutdown_token();
        let context_for_signal = self.context.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for ctrl-c");
            tracing::info!("Ctrl-C received — initiating graceful shutdown");
            context_for_signal.shutdown_runner(ShutdownReason::UserCancelled);
        });

        #[cfg(unix)]
        {
            let context_for_sigterm = self.context.clone();
            tokio::spawn(async move {
                let mut sigterm = tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::terminate(),
                )
                .expect("Failed to listen for SIGTERM");
                sigterm.recv().await;
                tracing::info!("SIGTERM received — initiating graceful shutdown");
                context_for_sigterm.shutdown_runner(ShutdownReason::OperatingSystemShutdown);
            });
        }

        // Set up the job dispatcher
        let mut job_dispatcher = JobDispatcher::new(self.context.clone());

        // Run-once channel
        let (run_once_tx, mut run_once_rx) = mpsc::channel::<bool>(1);
        if is_run_once {
            job_dispatcher.set_run_once_channel(run_once_tx);
        }

        // Choose V1 or V2 message loop
        let result = if is_v2_flow {
            self.run_v2_message_loop(
                &runner_settings,
                &job_dispatcher,
                is_run_once,
                &mut run_once_rx,
                shutdown_token.clone(),
            )
            .await
        } else {
            self.run_v1_message_loop(
                &runner_settings,
                &job_dispatcher,
                is_run_once,
                &mut run_once_rx,
                shutdown_token.clone(),
            )
            .await
        };

        // Shutdown dispatcher
        job_dispatcher.shutdown_async().await;

        result
    }

    // -----------------------------------------------------------------------
    // V1 message loop (legacy Actions service)
    // -----------------------------------------------------------------------

    /// V1 message loop using the Actions service long-poll API.
    async fn run_v1_message_loop(
        &self,
        runner_settings: &RunnerSettings,
        job_dispatcher: &JobDispatcher,
        is_run_once: bool,
        run_once_rx: &mut mpsc::Receiver<bool>,
        shutdown_token: CancellationToken,
    ) -> Result<i32> {
        let mut listener = MessageListener::new(self.context.clone());
        let mut error_throttler = ErrorThrottler::new();

        // Create session
        listener
            .create_session_async(shutdown_token.clone())
            .await
            .context("Failed to create V1 session")?;

        self.trace.info("V1 session created — entering message loop");
        println!(
            "√ Connected to GitHub\n\n{} Listening for Jobs",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%SZ")
        );

        // Main message loop
        loop {
            if shutdown_token.is_cancelled() {
                self.trace.info("Shutdown requested — exiting V1 message loop");
                break;
            }

            // Poll for the next message
            match listener.get_next_message_async(shutdown_token.clone()).await {
                Ok(Some(message)) => {
                    error_throttler.reset();

                    match message.type_kind() {
                        MessageType::JobRequest => {
                            self.trace.info("Received job request (V1)");
                            let raw_body = message.body.clone();
                            match serde_json::from_str::<AgentJobRequestMessage>(&message.body) {
                                Ok(job_request) => {
                                    if let Err(e) = job_dispatcher.run(&job_request, raw_body).await {
                                        self.trace.error(&format!(
                                            "Failed to dispatch job: {:?}",
                                            e
                                        ));
                                    }
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize job request: {}",
                                        e
                                    ));
                                }
                            }
                            // Delete the message after processing
                            let _ = listener.delete_message_async(&message).await;
                        }

                        MessageType::RunnerJobRequest => {
                            self.trace.info("Received RunnerJobRequest (V2 broker flow)");
                            match serde_json::from_str::<RunnerJobRequestRef>(&message.body) {
                                Ok(msg_ref) => {
                                    // 1. Acknowledge (best-effort)
                                    if msg_ref.should_acknowledge {
                                        if let Err(e) = listener.acknowledge_message_async(&msg_ref.runner_request_id).await {
                                            self.trace.warning(&format!(
                                                "Best-effort acknowledge failed: {}", e
                                            ));
                                        }
                                    }

                                    // 2. Acquire the full job from the run service
                                    let run_url = msg_ref.run_service_url.as_deref()
                                        .unwrap_or(&runner_settings.server_url);
                                    let billing_id = msg_ref.billing_owner_id.as_deref()
                                        .unwrap_or("");

                                    match self.acquire_job(
                                        &listener,
                                        run_url,
                                        &msg_ref.runner_request_id,
                                        billing_id,
                                    ).await {
                                        Ok((job_request, raw_body)) => {
                                            if let Err(e) = job_dispatcher.run(&job_request, raw_body).await {
                                                self.trace.error(&format!(
                                                    "Failed to dispatch V2 job: {:?}", e
                                                ));
                                            }
                                        }
                                        Err(e) => {
                                            self.trace.warning(&format!(
                                                "Failed to acquire job: {}", e
                                            ));
                                        }
                                    }
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize RunnerJobRequestRef: {}", e
                                    ));
                                }
                            }
                        }

                        MessageType::JobCancel => {
                            self.trace.info("Received job cancel (V1)");
                            match serde_json::from_str::<JobCancelMessage>(&message.body) {
                                Ok(cancel_msg) => {
                                    job_dispatcher.cancel(cancel_msg.job_id);
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize cancel message: {}",
                                        e
                                    ));
                                }
                            }
                            let _ = listener.delete_message_async(&message).await;
                        }

                        MessageType::AgentRefresh => {
                            self.trace.info("Received agent refresh message (V1 update)");
                            match serde_json::from_str::<AgentRefreshMessage>(&message.body) {
                                Ok(refresh_msg) => {
                                    if let Err(e) = self
                                        .handle_v1_update(
                                            &refresh_msg,
                                            runner_settings,
                                            shutdown_token.clone(),
                                        )
                                        .await
                                    {
                                        self.trace.error(&format!(
                                            "V1 self-update failed: {:?}",
                                            e
                                        ));
                                    } else {
                                        // Update was successful — exit for restart
                                        let _ = listener.delete_session_async().await;
                                        return Ok(constants::return_code::RUNNER_UPDATING);
                                    }
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize refresh message: {}",
                                        e
                                    ));
                                }
                            }
                            let _ = listener.delete_message_async(&message).await;
                        }

                        MessageType::RunnerRefresh => {
                            // V2-style refresh via V1 path (shouldn't happen often)
                            self.trace.info("Received runner refresh via V1 channel");
                            let _ = listener.delete_message_async(&message).await;
                        }

                        MessageType::JobMetadata | MessageType::BrokerMigration | MessageType::Unknown => {
                            self.trace.verbose(&format!(
                                "Ignoring message type: {}",
                                message.message_type
                            ));
                            let _ = listener.delete_message_async(&message).await;
                        }
                    }
                }

                Ok(None) => {
                    // No message — brief delay before next poll
                    tokio::select! {
                        _ = tokio::time::sleep(MESSAGE_POLL_DELAY) => {},
                        _ = shutdown_token.cancelled() => {},
                    }
                }

                Err(e) => {
                    self.trace.error(&format!(
                        "Error polling for V1 messages: {:?}",
                        e
                    ));
                    if !error_throttler
                        .increment_and_wait(shutdown_token.clone())
                        .await
                    {
                        break; // Cancelled during backoff
                    }
                }
            }

            // Check run-once completion
            if is_run_once {
                if let Ok(_completed) = run_once_rx.try_recv() {
                    self.trace
                        .info("Run-once job completed — exiting message loop");
                    let _ = listener.delete_session_async().await;
                    return Ok(constants::return_code::SUCCESS);
                }
            }
        }

        // Clean up session
        let _ = listener.delete_session_async().await;

        Ok(constants::return_code::SUCCESS)
    }

    // -----------------------------------------------------------------------
    // V2 message loop (broker)
    // -----------------------------------------------------------------------

    /// V2 message loop using the broker long-poll API.
    async fn run_v2_message_loop(
        &self,
        runner_settings: &RunnerSettings,
        job_dispatcher: &JobDispatcher,
        is_run_once: bool,
        run_once_rx: &mut mpsc::Receiver<bool>,
        shutdown_token: CancellationToken,
    ) -> Result<i32> {
        let mut listener = BrokerMessageListener::new(self.context.clone());
        let mut error_throttler = ErrorThrottler::new();

        // Create broker session
        listener
            .create_session_async(shutdown_token.clone())
            .await
            .context("Failed to create V2 broker session")?;

        self.trace.info("V2 broker session created — entering message loop");
        println!(
            "√ Connected to GitHub (V2)\n\n{} Listening for Jobs",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%SZ")
        );

        // Main message loop
        loop {
            if shutdown_token.is_cancelled() {
                self.trace.info("Shutdown requested — exiting V2 message loop");
                break;
            }

            match listener.get_next_message_async(shutdown_token.clone()).await {
                Ok(Some(message)) => {
                    error_throttler.reset();

                    match message.type_kind() {
                        BrokerMessageType::RunnerJobRequest => {
                            self.trace.info("Received job request (V2)");
                            match serde_json::from_str::<AgentJobRequestMessage>(&message.body) {
                                Ok(job_request) => {
                                    if let Err(e) = job_dispatcher.run(&job_request).await {
                                        self.trace.error(&format!(
                                            "Failed to dispatch V2 job: {:?}",
                                            e
                                        ));
                                    }
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize V2 job request: {}",
                                        e
                                    ));
                                }
                            }
                            let _ = listener.delete_message_async(&message).await;
                        }

                        BrokerMessageType::JobCancel => {
                            self.trace.info("Received job cancel (V2)");
                            match serde_json::from_str::<JobCancelMessage>(&message.body) {
                                Ok(cancel_msg) => {
                                    job_dispatcher.cancel(cancel_msg.job_id);
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize V2 cancel message: {}",
                                        e
                                    ));
                                }
                            }
                            let _ = listener.delete_message_async(&message).await;
                        }

                        BrokerMessageType::RunnerRefresh => {
                            self.trace.info("Received runner refresh (V2 update)");
                            match serde_json::from_str::<RunnerRefreshMessage>(&message.body) {
                                Ok(refresh_msg) => {
                                    if let Err(e) = self
                                        .handle_v2_update(
                                            &refresh_msg,
                                            runner_settings,
                                            shutdown_token.clone(),
                                        )
                                        .await
                                    {
                                        self.trace.error(&format!(
                                            "V2 self-update failed: {:?}",
                                            e
                                        ));
                                    } else {
                                        let _ = listener.delete_session_async().await;
                                        return Ok(constants::return_code::RUNNER_UPDATING);
                                    }
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize V2 refresh message: {}",
                                        e
                                    ));
                                }
                            }
                            let _ = listener.delete_message_async(&message).await;
                        }

                        BrokerMessageType::ForceTokenRefresh => {
                            self.trace.info("Received force token refresh (V2)");
                            // Token refresh is handled internally by the listener
                            let _ = listener.delete_message_async(&message).await;
                        }

                        BrokerMessageType::RunnerRefreshConfig => {
                            self.trace.info("Received config refresh (V2)");
                            match serde_json::from_str::<RunnerRefreshConfigMessage>(&message.body)
                            {
                                Ok(config_msg) => {
                                    let updater =
                                        RunnerConfigUpdater::new(self.context.clone());
                                    match updater.process_config_refresh(&config_msg) {
                                        Ok(true) => {
                                            self.trace.info(
                                                "Config refreshed — runner will restart",
                                            );
                                            let _ = listener.delete_session_async().await;
                                            return Ok(
                                                constants::return_code::RUNNER_CONFIGURATION_REFRESHED,
                                            );
                                        }
                                        Ok(false) => {
                                            self.trace.info("Config refresh — no changes");
                                        }
                                        Err(e) => {
                                            self.trace.error(&format!(
                                                "Config refresh failed: {:?}",
                                                e
                                            ));
                                        }
                                    }
                                }
                                Err(e) => {
                                    self.trace.error(&format!(
                                        "Failed to deserialize config refresh: {}",
                                        e
                                    ));
                                }
                            }
                            let _ = listener.delete_message_async(&message).await;
                        }

                        BrokerMessageType::HostedRunnerShutdown => {
                            self.trace.info("Received hosted runner shutdown (V2)");
                            self.context
                                .shutdown_runner(ShutdownReason::OperatingSystemShutdown);
                            let _ = listener.delete_message_async(&message).await;
                        }

                        BrokerMessageType::Unknown => {
                            self.trace.verbose(&format!(
                                "Ignoring unknown V2 message type: {}",
                                message.message_type
                            ));
                            let _ = listener.delete_message_async(&message).await;
                        }
                    }
                }

                Ok(None) => {
                    tokio::select! {
                        _ = tokio::time::sleep(MESSAGE_POLL_DELAY) => {},
                        _ = shutdown_token.cancelled() => {},
                    }
                }

                Err(e) => {
                    self.trace.error(&format!(
                        "Error polling V2 broker: {:?}",
                        e
                    ));
                    if !error_throttler
                        .increment_and_wait(shutdown_token.clone())
                        .await
                    {
                        break;
                    }
                }
            }

            // Check run-once completion
            if is_run_once {
                if let Ok(_completed) = run_once_rx.try_recv() {
                    self.trace
                        .info("Run-once job completed — exiting V2 message loop");
                    let _ = listener.delete_session_async().await;
                    return Ok(constants::return_code::SUCCESS);
                }
            }
        }

        let _ = listener.delete_session_async().await;
        Ok(constants::return_code::SUCCESS)
    }

    // -----------------------------------------------------------------------
    // -----------------------------------------------------------------------
    // Broker job acquisition
    // -----------------------------------------------------------------------

    /// Acquire the full job message from the run service (V2 broker flow).
    async fn acquire_job(
        &self,
        listener: &MessageListener,
        run_service_url: &str,
        runner_request_id: &str,
        billing_owner_id: &str,
    ) -> Result<(AgentJobRequestMessage, String)> {
        let access_token = listener.get_access_token()
            .ok_or_else(|| anyhow::anyhow!("No access token for acquire job"))?;

        let client = runner_common::HttpClientFactory::create_client(&self.context.web_proxy)?;

        let base = run_service_url.trim_end_matches('/');
        let url = format!("{}/acquirejob", base);

        let payload = AcquireJobRequest {
            job_message_id: runner_request_id.to_string(),
            runner_os: constants::CURRENT_PLATFORM.label_name().to_string(),
            billing_owner_id: billing_owner_id.to_string(),
        };

        self.trace.info(&format!("Acquiring job from: {}", url));

        let response = client
            .post(&url)
            .bearer_auth(&access_token)
            .header("Accept", "application/json;api-version=6.0-preview")
            .json(&payload)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("Failed to send acquire job request")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Acquire job failed with HTTP {}: {}",
                status.as_u16(),
                body
            ));
        }

        let body_text = response.text().await
            .context("Failed to read acquire job response")?;

        self.trace.info(&format!(
            "Acquired job response (first 500 chars): {}",
            &body_text[..body_text.len().min(500)]
        ));

        // DEBUG: dump the full acquired job JSON to a file for inspection
        {
            let diag_dir = self.context.get_directory(
                runner_common::constants::WellKnownDirectory::Diag,
            );
            let dump_path = diag_dir.join("acquired_job_body.json");
            if let Err(e) = std::fs::write(&dump_path, &body_text) {
                self.trace.warning(&format!(
                    "Failed to write acquired job dump to {:?}: {}", dump_path, e
                ));
            } else {
                self.trace.info(&format!(
                    "Full acquired job body written to {:?} ({} bytes)",
                    dump_path, body_text.len()
                ));
            }
        }

        let job_message: AgentJobRequestMessage = serde_json::from_str(&body_text)
            .context("Failed to deserialize acquired job message")?;

        Ok((job_message, body_text))
    }

    // -----------------------------------------------------------------------
    // Self-update handlers
    // -----------------------------------------------------------------------

    /// Handle a V1 self-update (AgentRefreshMessage).
    async fn handle_v1_update(
        &self,
        message: &AgentRefreshMessage,
        runner_settings: &RunnerSettings,
        cancel: CancellationToken,
    ) -> Result<()> {
        // Check if updates are disabled
        if runner_settings.disable_update {
            self.trace
                .info("Self-update is disabled — ignoring AgentRefreshMessage");
            return Ok(());
        }

        let updater = SelfUpdater::new(self.context.clone());

        if !updater.needs_update(&message.target_version) {
            return Ok(());
        }

        let update_dir = updater
            .download_latest_runner(
                &message.target_version,
                message.download_url.as_deref(),
                cancel,
            )
            .await?;

        let _script = updater.generate_update_script(&update_dir)?;

        self.trace.info("V1 self-update prepared — runner will restart");
        Ok(())
    }

    /// Handle a V2 self-update (RunnerRefreshMessage).
    async fn handle_v2_update(
        &self,
        message: &RunnerRefreshMessage,
        runner_settings: &RunnerSettings,
        cancel: CancellationToken,
    ) -> Result<()> {
        if runner_settings.disable_update {
            self.trace
                .info("Self-update is disabled — ignoring RunnerRefreshMessage");
            return Ok(());
        }

        let updater = SelfUpdaterV2::new(self.context.clone());

        if !updater.needs_update(&message.target_version) {
            return Ok(());
        }

        let update_dir = updater.download_and_verify(message, cancel).await?;

        let _script = updater.generate_update_script(&update_dir)?;

        self.trace.info("V2 self-update prepared — runner will restart");
        Ok(())
    }
}
