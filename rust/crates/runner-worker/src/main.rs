// Entry point for the GitHub Actions Runner Worker process.
// Maps the C# `Program.cs` / `Worker.cs` entry point.
//
// The worker is spawned by the listener with `--pipeIn <path> --pipeOut <path>` arguments.
// It receives a job message over the IPC pipe, executes all steps, and exits with
// a return code that encodes the `TaskResult`.

use anyhow::{Context, Result};
use clap::Parser;
use runner_common::host_context::HostContext;
use runner_common::util::task_result_util::{TaskResult, TaskResultUtil};
use std::sync::Arc;

use runner_worker::worker::Worker;

/// Command-line arguments for the worker process.
#[derive(Parser, Debug)]
#[command(name = "Runner.Worker", about = "GitHub Actions Runner Worker")]
struct Args {
    /// Path to the IPC socket/pipe for receiving messages from the listener.
    #[arg(long = "pipeIn")]
    pipe_in: String,

    /// Path to the IPC socket/pipe for sending messages to the listener.
    #[arg(long = "pipeOut")]
    pipe_out: String,
}

fn main() {
    let args = Args::parse();

    // Build the async runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build Tokio runtime");

    let exit_code = runtime.block_on(async move { run(args).await });

    std::process::exit(exit_code);
}

async fn run(args: Args) -> i32 {
    // Initialize tracing subscriber for diagnostics
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("Worker process starting.");
    tracing::info!("  pipeIn  = {}", args.pipe_in);
    tracing::info!("  pipeOut = {}", args.pipe_out);

    // Create the host context for the worker process
    let host_context = HostContext::new("Worker");

    // Create the worker service
    let worker = Worker::new(Arc::clone(&host_context));

    // Run the worker – returns a TaskResult
    match worker.run_async(&args.pipe_in, &args.pipe_out).await {
        Ok(result) => {
            let return_code = TaskResultUtil::translate_to_return_code(result);
            tracing::info!(
                "Worker completed with result {} (return code {})",
                result,
                return_code
            );
            return_code
        }
        Err(e) => {
            tracing::error!("Worker failed with error: {:#}", e);
            TaskResultUtil::translate_to_return_code(TaskResult::Failed)
        }
    }
}
