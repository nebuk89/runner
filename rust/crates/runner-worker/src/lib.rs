// runner-worker: Job and step execution engine for the GitHub Actions Runner.
// This crate maps the C# `Runner.Worker` project and depends on `runner-sdk` and `runner-common`.
//
// Architecture:
//   Worker::run_async → JobRunner::run_async → JobExtension::initialize_job
//     → StepsRunner::run_async → per-step Handler::run_async

pub mod action_command_manager;
pub mod action_manager;
pub mod action_manifest_manager;
pub mod condition_trace_writer;
pub mod container;
pub mod execution_context;
pub mod expressions;
pub mod feature_manager;
pub mod file_command_manager;
pub mod github_context;
pub mod handlers;
pub mod issue_matcher;
pub mod job_extension;
pub mod job_runner;
pub mod results_client;
pub mod run_server;
pub mod runner_context;
pub mod steps_context;
pub mod steps_runner;
pub mod tracking_manager;
pub mod variables;
pub mod worker;
