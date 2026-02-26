// runner-plugin-host: Entry point for out-of-process plugin execution.
//
// Maps `Runner.PluginHost/Program.cs` from the C# codebase.
//
// Usage:
//   Runner.PluginHost action <plugin-name>
//
// The execution context JSON is read from stdin (one line).
// All output is written to stdout using `##[...]` action commands so the
// runner worker can parse trace / error messages.

use anyhow::{Context, Result};
use runner_plugins::{DownloadArtifactPlugin, PublishArtifactPlugin};
use runner_sdk::{ActionPlugin, ActionPluginContext, StringUtil, TraceWriter};
use std::io::BufRead;
use std::process::ExitCode;

// ---------------------------------------------------------------------------
// Stdout-based trace writer that emits action commands
// ---------------------------------------------------------------------------

/// A trace writer that writes to stdout using the `##[...]` command protocol.
///
/// The runner worker process captures these lines and maps them to the
/// appropriate log levels.
struct PluginTraceWriter {
    debug_enabled: bool,
}

impl PluginTraceWriter {
    fn new(debug_enabled: bool) -> Self {
        Self { debug_enabled }
    }
}

impl TraceWriter for PluginTraceWriter {
    fn info(&self, message: &str) {
        let escaped = escape(message);
        // Info-level messages are emitted via the debug command when debug is
        // enabled; otherwise they go through as plain output so the worker
        // picks them up.
        if self.debug_enabled {
            println!("##[debug]{escaped}");
        } else {
            println!("{escaped}");
        }
    }

    fn verbose(&self, message: &str) {
        if self.debug_enabled {
            for line in message.replace("\r\n", "\n").split('\n') {
                println!("##[debug]{}", escape(line));
            }
        }
    }

    fn warning(&self, message: &str) {
        println!("##[warning]{}", escape(message));
    }

    fn error(&self, message: &str) {
        println!("##[error]{}", escape(message));
    }
}

// ---------------------------------------------------------------------------
// Command escaping (mirrors C# RunnerActionPluginExecutionContext.Escape)
// ---------------------------------------------------------------------------

fn escape(input: &str) -> String {
    input
        .replace(';', "%3B")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace(']', "%5D")
}

// ---------------------------------------------------------------------------
// Plugin registry
// ---------------------------------------------------------------------------

/// Resolve a plugin implementation by its fully-qualified type name.
///
/// The C# host uses reflection (`Type.GetType`) to instantiate the plugin.
/// In Rust we use a simple match against known type names. The names match the
/// fully-qualified C# type names for backwards compatibility with the worker
/// which passes these names as arguments.
fn resolve_plugin(type_name: &str) -> Option<Box<dyn ActionPlugin>> {
    // Normalise: the worker may pass the full assembly-qualified name
    // e.g. "GitHub.Runner.Plugins.Artifact.PublishArtifact, Runner.Plugins"
    // We match on the type portion before the comma.
    let normalized = type_name
        .split(',')
        .next()
        .unwrap_or(type_name)
        .trim();

    match normalized {
        // Full C# type names
        "GitHub.Runner.Plugins.Artifact.PublishArtifact" => {
            Some(Box::new(PublishArtifactPlugin))
        }
        "GitHub.Runner.Plugins.Artifact.DownloadArtifact" => {
            Some(Box::new(DownloadArtifactPlugin))
        }
        // Short names for convenience
        "PublishArtifact" => Some(Box::new(PublishArtifactPlugin)),
        "DownloadArtifact" => Some(Box::new(DownloadArtifactPlugin)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    // Install ctrl-c handler – on Ctrl+C we just exit with code 1.
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let cancel = cancel.clone();
        let _ = ctrlc::set_handler(move || {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }

    match run_plugin() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_plugin() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Expect exactly: <binary> <plugin_type> <assembly_qualified_name>
    if args.len() != 3 {
        anyhow::bail!(
            "Usage: Runner.PluginHost <plugin_type> <assembly_qualified_name>\n\
             Expected 2 arguments, got {}",
            args.len() - 1,
        );
    }

    let plugin_type = &args[1];
    let assembly_qualified_name = &args[2];

    if !plugin_type.eq_ignore_ascii_case("action") {
        anyhow::bail!("Unsupported plugin type: {plugin_type}");
    }

    if assembly_qualified_name.is_empty() {
        anyhow::bail!("Assembly qualified name must not be empty");
    }

    // Read the serialized execution context from stdin (one line).
    let stdin = std::io::stdin();
    let serialized_context = {
        let mut line = String::new();
        stdin
            .lock()
            .read_line(&mut line)
            .context("Failed to read execution context from stdin")?;
        line.trim_end().to_string()
    };

    if serialized_context.is_empty() {
        anyhow::bail!("Execution context from stdin must not be empty");
    }

    let mut execution_context: ActionPluginContext =
        StringUtil::convert_from_json(&serialized_context)
            .context("Failed to deserialize execution context")?;

    // Determine debug mode from the context variables.
    let debug_enabled = execution_context.is_debug();
    let trace = PluginTraceWriter::new(debug_enabled);

    // Resolve the plugin by name.
    let plugin = resolve_plugin(assembly_qualified_name).ok_or_else(|| {
        anyhow::anyhow!("Unknown plugin type: {assembly_qualified_name}")
    })?;

    // Build the tokio runtime and execute the plugin.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")?;

    let result = runtime.block_on(async {
        plugin.run(&mut execution_context, &trace).await
    });

    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            // Any exception from the plugin fails the task – emit an error
            // command so the worker marks the step as failed.
            trace.error(&format!("{e:#}"));
            if debug_enabled {
                // In debug mode, also emit the full error chain.
                trace.verbose(&format!("{e:?}"));
            }
            anyhow::bail!("Plugin execution failed: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_special_chars() {
        assert_eq!(escape("a;b"), "a%3Bb");
        assert_eq!(escape("line1\r\nline2"), "line1%0D%0Aline2");
        assert_eq!(escape("msg]end"), "msg%5Dend");
    }

    #[test]
    fn escape_no_special_chars() {
        assert_eq!(escape("hello world"), "hello world");
    }

    #[test]
    fn resolve_known_plugins() {
        assert!(resolve_plugin("GitHub.Runner.Plugins.Artifact.PublishArtifact").is_some());
        assert!(resolve_plugin("GitHub.Runner.Plugins.Artifact.DownloadArtifact").is_some());
        assert!(resolve_plugin("PublishArtifact").is_some());
        assert!(resolve_plugin("DownloadArtifact").is_some());
    }

    #[test]
    fn resolve_with_assembly_qualifier() {
        let full = "GitHub.Runner.Plugins.Artifact.PublishArtifact, Runner.Plugins";
        assert!(resolve_plugin(full).is_some());
    }

    #[test]
    fn resolve_unknown_plugin() {
        assert!(resolve_plugin("NoSuchPlugin").is_none());
    }

    #[test]
    fn plugin_trace_writer_output() {
        // Just verify construction doesn't panic
        let _w = PluginTraceWriter::new(true);
        let _w2 = PluginTraceWriter::new(false);
    }
}
