// Tracing infrastructure mapping `Tracing.cs` and `TraceManager.cs`.
// Provides per-component trace sources with secret masking integration.

use crate::secret_masker::SecretMasker;
use chrono::Utc;
use runner_sdk::TraceWriter;
use std::sync::Arc;

/// Trace event severity level, mirroring C# `TraceEventType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceEventType {
    Verbose,
    Information,
    Warning,
    Error,
    Critical,
}

impl std::fmt::Display for TraceEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceEventType::Verbose => write!(f, "VERB"),
            TraceEventType::Information => write!(f, "INFO"),
            TraceEventType::Warning => write!(f, "WARN"),
            TraceEventType::Error => write!(f, "ERR "),
            TraceEventType::Critical => write!(f, "CRIT"),
        }
    }
}

/// Configuration for trace output.
#[derive(Debug, Clone)]
pub struct TraceSetting {
    /// Minimum severity level to emit.
    pub level: TraceEventType,
    /// Whether to also print to stdout.
    pub print_to_stdout: bool,
}

impl Default for TraceSetting {
    fn default() -> Self {
        Self {
            level: TraceEventType::Verbose,
            print_to_stdout: false,
        }
    }
}

/// A trace source that masks secrets before emitting log lines.
///
/// Maps `Tracing` in the C# runner. Each component gets its own `Tracing`
/// instance with a specific name, but they all share the same `SecretMasker`.
#[derive(Clone)]
pub struct Tracing {
    name: String,
    secret_masker: Arc<SecretMasker>,
    setting: TraceSetting,
}

impl Tracing {
    /// Create a new `Tracing` instance.
    pub fn new(name: impl Into<String>, secret_masker: Arc<SecretMasker>, setting: TraceSetting) -> Self {
        Self {
            name: name.into(),
            secret_masker,
            setting,
        }
    }

    /// Log a message at the given severity level.
    fn trace(&self, event_type: TraceEventType, message: &str) {
        // Check level threshold
        if (event_type as u8) < (self.setting.level as u8) {
            return;
        }

        let masked = self.secret_masker.mask_secrets(message);
        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let formatted = format!("[{}][{}] {}: {}", timestamp, &self.name, event_type, masked);

        // Dispatch to the tracing crate
        match event_type {
            TraceEventType::Error | TraceEventType::Critical => {
                tracing::error!("{}", formatted);
            }
            TraceEventType::Warning => {
                tracing::warn!("{}", formatted);
            }
            TraceEventType::Information => {
                tracing::info!("{}", formatted);
            }
            TraceEventType::Verbose => {
                tracing::debug!("{}", formatted);
            }
        }

        if self.setting.print_to_stdout {
            println!("{}", formatted);
        }
    }

    /// Get the name of this trace source.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Log an entering-function trace message.
    pub fn entering(&self, name: &str) {
        self.verbose(&format!("Entering {}", name));
    }

    /// Log a leaving-function trace message.
    pub fn leaving(&self, name: &str) {
        self.verbose(&format!("Leaving {}", name));
    }

    /// Log a serialized object as verbose JSON.
    pub fn verbose_object<T: serde::Serialize>(&self, item: &T) {
        match serde_json::to_string_pretty(item) {
            Ok(json) => self.verbose(&json),
            Err(e) => self.verbose(&format!("<serialization error: {}>", e)),
        }
    }

    /// Log a serialized object as info JSON.
    pub fn info_object<T: serde::Serialize>(&self, item: &T) {
        match serde_json::to_string_pretty(item) {
            Ok(json) => self.info(&json),
            Err(e) => self.info(&format!("<serialization error: {}>", e)),
        }
    }

    /// Log an error object (Display + Debug).
    pub fn error_err(&self, err: &dyn std::error::Error) {
        self.error(&format!("{}", err));
        let mut source = err.source();
        while let Some(cause) = source {
            self.error("#####################################################");
            self.error(&format!("{}", cause));
            source = cause.source();
        }
    }
}

impl TraceWriter for Tracing {
    fn info(&self, message: &str) {
        self.trace(TraceEventType::Information, message);
    }

    fn verbose(&self, message: &str) {
        self.trace(TraceEventType::Verbose, message);
    }

    fn warning(&self, message: &str) {
        self.trace(TraceEventType::Warning, message);
    }

    fn error(&self, message: &str) {
        self.trace(TraceEventType::Error, message);
    }
}

/// Manages trace sources across the application. Each source is identified
/// by a string name and shares the same `SecretMasker`.
pub struct TraceManager {
    secret_masker: Arc<SecretMasker>,
    default_setting: TraceSetting,
}

impl TraceManager {
    /// Create a new `TraceManager` with the given `SecretMasker`.
    pub fn new(secret_masker: Arc<SecretMasker>) -> Self {
        Self {
            secret_masker,
            default_setting: TraceSetting::default(),
        }
    }

    /// Create a new `TraceManager` with a specific setting.
    pub fn with_setting(secret_masker: Arc<SecretMasker>, setting: TraceSetting) -> Self {
        Self {
            secret_masker,
            default_setting: setting,
        }
    }

    /// Get (create) a named trace source.
    pub fn get(&self, name: &str) -> Tracing {
        Tracing::new(name, self.secret_masker.clone(), self.default_setting.clone())
    }

    /// Access the underlying secret masker.
    pub fn secret_masker(&self) -> &Arc<SecretMasker> {
        &self.secret_masker
    }
}
