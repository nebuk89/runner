// runner-sdk: Foundation layer for the GitHub Actions Runner.
// This crate has ZERO dependencies on other runner crates and provides
// core utilities, traits, and abstractions used throughout the runner.

pub mod action_plugin;
pub mod arg_util;
pub mod build_constants;
pub mod io_util;
pub mod path_util;
pub mod process_invoker;
pub mod string_util;
pub mod trace;
pub mod url_util;
pub mod vss_util;
pub mod web_proxy;
pub mod which_util;

// Re-export commonly used items at crate root
pub use action_plugin::{ActionPlugin, ActionPluginContext};
pub use arg_util::ArgUtil;
pub use build_constants::{RunnerPackage, Source};
pub use io_util::IOUtil;
pub use path_util::PathUtil;
pub use process_invoker::{ProcessDataReceivedEventArgs, ProcessExitCodeError, ProcessInvoker};
pub use string_util::StringUtil;
pub use trace::TraceWriter;
pub use url_util::UrlUtil;
pub use vss_util::VssUtil;
pub use web_proxy::RunnerWebProxy;
pub use which_util::WhichUtil;
