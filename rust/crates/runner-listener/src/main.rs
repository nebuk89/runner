// Entry point for the GitHub Actions Runner Listener process.
// Maps the C# `Program.cs` entry point.
//
// The listener is the main runner process. It parses CLI args, validates the
// platform, creates a HostContext, and delegates to the Runner orchestrator.

use runner_common::constants;
use runner_common::host_context::HostContext;
use std::sync::Arc;

use runner_listener::runner::Runner;

/// Load the runner version string from the `runnerversion` file next to the binary,
/// falling back to the Cargo package version.
fn load_runner_version() -> String {
    // Try to read from the file next to the binary
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(bin_dir) = exe_path.parent() {
            let version_file = bin_dir.join("runnerversion");
            if let Ok(version) = std::fs::read_to_string(&version_file) {
                let trimmed = version.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
            // Also try root (parent of bin)
            if let Some(root_dir) = bin_dir.parent() {
                let version_file = root_dir.join("runnerversion");
                if let Ok(version) = std::fs::read_to_string(&version_file) {
                    let trimmed = version.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
            }
        }
    }

    // Fallback to Cargo package version
    runner_sdk::build_constants::RunnerPackage::VERSION.to_string()
}

/// Validate the current platform is supported.
fn validate_platform() {
    match constants::CURRENT_PLATFORM {
        constants::OsPlatform::Linux
        | constants::OsPlatform::MacOS
        | constants::OsPlatform::Windows => {}
    }
}

fn main() {
    validate_platform();

    let _version = load_runner_version();

    // Build the async runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build Tokio runtime");

    let exit_code = runtime.block_on(async move { run().await });

    std::process::exit(exit_code);
}

async fn run() -> i32 {
    // Initialize tracing subscriber for diagnostics
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("Runner listener process starting.");
    tracing::info!(
        "  Version = {}",
        runner_sdk::build_constants::RunnerPackage::VERSION
    );
    tracing::info!(
        "  Commit  = {}",
        runner_sdk::build_constants::Source::COMMIT_HASH
    );
    tracing::info!(
        "  Platform = {} / {}",
        constants::CURRENT_PLATFORM,
        constants::CURRENT_ARCHITECTURE
    );

    // Create the host context for the listener process
    let host_context = HostContext::new("Runner");
    host_context.load_default_user_agents();

    // Create and run the runner orchestrator
    let runner = Runner::new(Arc::clone(&host_context));

    match runner.execute_command().await {
        Ok(exit_code) => {
            tracing::info!("Runner exiting with code {}", exit_code);
            exit_code
        }
        Err(e) => {
            tracing::error!("Runner failed with error: {:?}", e);
            constants::return_code::TERMINATED_ERROR
        }
    }
}
