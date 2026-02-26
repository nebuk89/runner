/// Trace / logging abstraction mapping `ITraceWriter.cs`.
///
/// The C# runner uses `ITraceWriter` as a lightweight interface for diagnostic
/// output. We mirror that with a Rust trait.
pub trait TraceWriter: Send + Sync {
    /// Log an informational message.
    fn info(&self, message: &str);

    /// Log a verbose / debug message.
    fn verbose(&self, message: &str);

    /// Log a warning message.
    fn warning(&self, message: &str) {
        self.info(&format!("##[warning]{message}"));
    }

    /// Log an error message.
    fn error(&self, message: &str) {
        self.info(&format!("##[error]{message}"));
    }
}

/// A simple trace writer that prints to the `tracing` crate at appropriate levels.
#[derive(Debug, Clone)]
pub struct TracingTraceWriter;

impl TraceWriter for TracingTraceWriter {
    fn info(&self, message: &str) {
        tracing::info!("{}", message);
    }

    fn verbose(&self, message: &str) {
        tracing::debug!("{}", message);
    }

    fn warning(&self, message: &str) {
        tracing::warn!("{}", message);
    }

    fn error(&self, message: &str) {
        tracing::error!("{}", message);
    }
}

/// A no-op trace writer that discards all messages. Useful for tests.
#[derive(Debug, Clone)]
pub struct NullTraceWriter;

impl TraceWriter for NullTraceWriter {
    fn info(&self, _message: &str) {}
    fn verbose(&self, _message: &str) {}
    fn warning(&self, _message: &str) {}
    fn error(&self, _message: &str) {}
}

/// A trace writer that collects all messages into a `Vec`.
/// Useful for testing output.
#[derive(Debug)]
pub struct CollectingTraceWriter {
    messages: parking_lot::Mutex<Vec<(TraceLevel, String)>>,
}

/// The level of a collected trace message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceLevel {
    Info,
    Verbose,
    Warning,
    Error,
}

impl CollectingTraceWriter {
    pub fn new() -> Self {
        Self {
            messages: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Return all collected messages.
    pub fn messages(&self) -> Vec<(TraceLevel, String)> {
        self.messages.lock().clone()
    }

    /// Clear collected messages.
    pub fn clear(&self) {
        self.messages.lock().clear();
    }
}

impl Default for CollectingTraceWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceWriter for CollectingTraceWriter {
    fn info(&self, message: &str) {
        self.messages
            .lock()
            .push((TraceLevel::Info, message.to_string()));
    }

    fn verbose(&self, message: &str) {
        self.messages
            .lock()
            .push((TraceLevel::Verbose, message.to_string()));
    }

    fn warning(&self, message: &str) {
        self.messages
            .lock()
            .push((TraceLevel::Warning, message.to_string()));
    }

    fn error(&self, message: &str) {
        self.messages
            .lock()
            .push((TraceLevel::Error, message.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collecting_writer() {
        let writer = CollectingTraceWriter::new();
        writer.info("hello");
        writer.warning("warn");
        writer.error("err");
        writer.verbose("verb");
        let msgs = writer.messages();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0], (TraceLevel::Info, "hello".into()));
        assert_eq!(msgs[1], (TraceLevel::Warning, "warn".into()));
        assert_eq!(msgs[2], (TraceLevel::Error, "err".into()));
        assert_eq!(msgs[3], (TraceLevel::Verbose, "verb".into()));
    }

    #[test]
    fn null_writer_does_not_panic() {
        let writer = NullTraceWriter;
        writer.info("test");
        writer.verbose("test");
        writer.warning("test");
        writer.error("test");
    }
}
