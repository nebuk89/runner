// runner-listener: Main entry point and message loop for the GitHub Actions Runner.
// This crate maps the C# `Runner.Listener` project and depends on `runner-sdk` and `runner-common`.
//
// Architecture:
//   main → Runner::execute_command → configure / remove / run / warmup / check / help / version
//   Runner::run_async → MessageListener/BrokerMessageListener → JobDispatcher → Worker

pub mod broker_message_listener;
pub mod checks;
pub mod command_settings;
pub mod configuration;
pub mod error_throttler;
pub mod job_dispatcher;
pub mod message_listener;
pub mod runner;
pub mod runner_config_updater;
pub mod self_updater;
pub mod self_updater_v2;
