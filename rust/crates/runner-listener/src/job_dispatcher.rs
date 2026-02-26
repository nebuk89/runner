// JobDispatcher mapping `JobDispatcher.cs`.
// Spawns worker child processes via ProcessInvoker, communicates via ProcessChannel (IPC),
// manages run/cancel/wait lifecycle.

use anyhow::{Context, Result};
use runner_common::constants::{self, WellKnownDirectory};
use runner_common::host_context::HostContext;
use runner_common::process_channel::ProcessChannel;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Job request types (serialised from server messages)
// ---------------------------------------------------------------------------

/// A job request received from the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentJobRequestMessage {
    #[serde(rename = "jobId")]
    pub job_id: Uuid,
    #[serde(default, rename = "jobDisplayName")]
    pub job_display_name: String,
    #[serde(default, rename = "requestId")]
    pub request_id: u64,
    #[serde(default, rename = "lockedUntil")]
    pub locked_until: String,
    #[serde(default, rename = "plan")]
    pub plan: Option<serde_json::Value>,
    #[serde(default, rename = "timeline")]
    pub timeline: Option<serde_json::Value>,
    #[serde(default, rename = "resources")]
    pub resources: Option<serde_json::Value>,
    #[serde(default, rename = "variables")]
    pub variables: Option<serde_json::Value>,
    #[serde(default, rename = "steps")]
    pub steps: Option<serde_json::Value>,
    #[serde(default, rename = "contextData")]
    pub context_data: Option<serde_json::Value>,
}

/// A job cancel message received from the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCancelMessage {
    #[serde(rename = "jobId")]
    pub job_id: Uuid,
    #[serde(default)]
    pub timeout: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// WorkerDispatchInfo - tracks a running worker
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct WorkerDispatchInfo {
    job_id: Uuid,
    request_id: u64,
    cancel_token: CancellationToken,
    worker_handle: Option<JoinHandle<Result<i32>>>,
}

// ---------------------------------------------------------------------------
// JobDispatcher
// ---------------------------------------------------------------------------

/// Manages spawning worker processes to execute jobs.
///
/// Maps `JobDispatcher` in the C# runner. Each incoming job request
/// causes a new worker process to be spawned. The dispatcher communicates
/// with the worker via IPC (Unix domain sockets) using `ProcessChannel`.
pub struct JobDispatcher {
    context: Arc<HostContext>,
    trace: Tracing,
    /// Currently running workers, keyed by job ID.
    workers: Arc<Mutex<HashMap<Uuid, WorkerDispatchInfo>>>,
    /// Whether the dispatcher is busy (has at least one running worker).
    is_busy: Arc<Mutex<bool>>,
    /// Channel to signal that a run-once job has completed.
    run_once_tx: Option<mpsc::Sender<bool>>,
    /// Cancellation token for the overall dispatcher.
    #[allow(dead_code)]
    shutdown_token: CancellationToken,
}

impl JobDispatcher {
    /// Create a new `JobDispatcher`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("JobDispatcher");
        let shutdown_token = context.runner_shutdown_token();
        Self {
            context,
            trace,
            workers: Arc::new(Mutex::new(HashMap::new())),
            is_busy: Arc::new(Mutex::new(false)),
            run_once_tx: None,
            shutdown_token,
        }
    }

    /// Set the channel used to notify that a run-once job has completed.
    pub fn set_run_once_channel(&mut self, tx: mpsc::Sender<bool>) {
        self.run_once_tx = Some(tx);
    }

    /// Whether the dispatcher currently has any running worker.
    pub fn is_busy(&self) -> bool {
        *self.is_busy.lock().unwrap()
    }

    /// Dispatch a job request to a new worker process.
    pub async fn run(&self, job_request: &AgentJobRequestMessage) -> Result<()> {
        let job_id = job_request.job_id;

        self.trace.info(&format!(
            "Dispatching job {} (request_id={}): {}",
            job_id, job_request.request_id, job_request.job_display_name
        ));

        // Check if already running
        {
            let workers = self.workers.lock().unwrap();
            if workers.contains_key(&job_id) {
                self.trace.warning(&format!(
                    "Job {} is already running — ignoring duplicate dispatch",
                    job_id
                ));
                return Ok(());
            }
        }

        // Set busy
        *self.is_busy.lock().unwrap() = true;

        // Create IPC channel
        let mut channel = ProcessChannel::new();
        // Use /tmp for socket path to avoid exceeding macOS SUN_LEN limit (104 chars)
        // on Unix domain socket paths. The _work/_temp directory is often too deep.
        let socket_dir = std::path::PathBuf::from("/tmp");
        let socket_path = channel
            .start_server(&socket_dir)
            .context("Failed to create IPC channel for worker")?;

        self.trace.info(&format!(
            "IPC channel created at: {}",
            socket_path
        ));

        // Locate the worker binary
        let worker_binary = self.find_worker_binary()?;

        // Serialize the job request to send to the worker
        let job_body = serde_json::to_string(job_request)
            .context("Failed to serialize job request for worker")?;

        let cancel_token = CancellationToken::new();
        let cancel_for_task = cancel_token.clone();
        let context_clone = self.context.clone();
        let workers_clone = self.workers.clone();
        let is_busy_clone = self.is_busy.clone();
        let run_once_tx = self.run_once_tx.clone();
        let trace_clone = self.trace.clone();
        let worker_binary_clone = worker_binary.clone();
        let socket_path_clone = socket_path.clone();

        // Spawn the worker in a background task
        let handle: JoinHandle<Result<i32>> = tokio::spawn(async move {
            let result = Self::run_worker(
                context_clone,
                trace_clone.clone(),
                worker_binary_clone,
                socket_path_clone,
                job_body,
                channel,
                cancel_for_task,
            )
            .await;

            // Clean up
            {
                let mut workers = workers_clone.lock().unwrap();
                workers.remove(&job_id);
                if workers.is_empty() {
                    *is_busy_clone.lock().unwrap() = false;
                }
            }

            // Notify run-once completion
            if let Some(tx) = &run_once_tx {
                let completed = result.is_ok();
                let _ = tx.send(completed).await;
            }

            match &result {
                Ok(exit_code) => {
                    trace_clone.info(&format!(
                        "Worker for job {} exited with code {}",
                        job_id, exit_code
                    ));
                }
                Err(e) => {
                    trace_clone.error(&format!(
                        "Worker for job {} failed: {:?}",
                        job_id, e
                    ));
                }
            }

            result
        });

        // Store the worker info
        {
            let mut workers = self.workers.lock().unwrap();
            workers.insert(
                job_id,
                WorkerDispatchInfo {
                    job_id,
                    request_id: job_request.request_id,
                    cancel_token,
                    worker_handle: Some(handle),
                },
            );
        }

        Ok(())
    }

    /// Run the worker process and communicate via IPC.
    async fn run_worker(
        _context: Arc<HostContext>,
        trace: Tracing,
        worker_binary: PathBuf,
        socket_path: String,
        job_body: String,
        mut channel: ProcessChannel,
        cancel: CancellationToken,
    ) -> Result<i32> {
        trace.info(&format!(
            "Starting worker process: {:?} --pipeIn {} --pipeOut {}",
            worker_binary, socket_path, socket_path
        ));

        let mut child = tokio::process::Command::new(&worker_binary)
            .arg("--pipeIn")
            .arg(&socket_path)
            .arg("--pipeOut")
            .arg(&socket_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn worker process")?;

        trace.info(&format!(
            "Worker process spawned with PID: {}",
            child.id().unwrap_or(0)
        ));

        // Wait for the worker to connect to the IPC socket
        trace.info("Waiting for worker to connect to IPC socket...");
        channel.accept().await.context("Failed to accept worker IPC connection")?;
        trace.info("Worker connected to IPC socket");

        // Send the job request to the worker
        trace.info("Sending job request to worker via IPC...");
        channel
            .send_async(
                runner_common::process_channel::MessageType::NewJobRequest,
                &job_body,
            )
            .await
            .context("Failed to send job request to worker via IPC")?;
        trace.info("Job request sent to worker");

        // Wait for the worker to finish or for cancellation
        let exit_code = tokio::select! {
            status = child.wait() => {
                let status = status.context("Failed to wait for worker process")?;
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    status.code().or_else(|| status.signal().map(|s| 128 + s)).unwrap_or(1)
                }
                #[cfg(not(unix))]
                {
                    status.code().unwrap_or(1)
                }
            }
            _ = cancel.cancelled() => {
                trace.info("Worker cancellation requested — sending kill signal");
                let _ = child.kill().await;
                let _ = child.wait().await;
                constants::return_code::TERMINATED_ERROR
            }
        };

        Ok(exit_code)
    }

    /// Cancel a running job.
    pub fn cancel(&self, job_id: Uuid) {
        let workers = self.workers.lock().unwrap();
        if let Some(info) = workers.get(&job_id) {
            self.trace.info(&format!("Cancelling job {}", job_id));
            info.cancel_token.cancel();
        } else {
            self.trace.warning(&format!(
                "Cannot cancel job {} — not found in running workers",
                job_id
            ));
        }
    }

    /// Wait for a specific job to complete. Returns the exit code.
    pub async fn wait_async(&self, job_id: Uuid) -> Result<i32> {
        let handle = {
            let mut workers = self.workers.lock().unwrap();
            workers
                .get_mut(&job_id)
                .and_then(|info| info.worker_handle.take())
        };

        match handle {
            Some(h) => {
                let result = h.await.context("Worker task panicked")?;
                result
            }
            None => Err(anyhow::anyhow!("Job {} is not running or already waited", job_id)),
        }
    }

    /// Shut down the dispatcher, cancelling all running workers.
    pub async fn shutdown_async(&self) {
        self.trace.info("Shutting down job dispatcher");

        let job_ids: Vec<Uuid> = {
            let workers = self.workers.lock().unwrap();
            workers.keys().cloned().collect()
        };

        for job_id in &job_ids {
            self.cancel(*job_id);
        }

        // Wait briefly for workers to exit
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Force kill any remaining
        let remaining: Vec<Uuid> = {
            let workers = self.workers.lock().unwrap();
            workers.keys().cloned().collect()
        };

        if !remaining.is_empty() {
            self.trace.warning(&format!(
                "Force-killing {} remaining worker(s)",
                remaining.len()
            ));
        }
    }

    /// Find the worker binary path.
    fn find_worker_binary(&self) -> Result<PathBuf> {
        let bin_dir = self.context.get_directory(WellKnownDirectory::Bin);

        // Check for Runner.Worker binary (the Rust binary name)
        let worker_path = bin_dir.join("Runner.Worker");
        if worker_path.exists() {
            return Ok(worker_path);
        }

        // Check Windows variant
        let worker_path_exe = bin_dir.join("Runner.Worker.exe");
        if worker_path_exe.exists() {
            return Ok(worker_path_exe);
        }

        // Fallback: look next to our own binary
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(dir) = exe_path.parent() {
                let worker = dir.join("Runner.Worker");
                if worker.exists() {
                    return Ok(worker);
                }
                let worker_exe = dir.join("Runner.Worker.exe");
                if worker_exe.exists() {
                    return Ok(worker_exe);
                }
            }
        }

        Err(anyhow::anyhow!(
            "Worker binary not found in {:?}",
            bin_dir
        ))
    }

    /// Get the list of currently running job IDs.
    pub fn running_job_ids(&self) -> Vec<Uuid> {
        let workers = self.workers.lock().unwrap();
        workers.keys().cloned().collect()
    }
}
