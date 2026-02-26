// Custom exception types mapping `Exceptions.cs`.

use std::fmt;

/// An error indicating that the operation should NOT be retried.
/// This is used to distinguish between transient failures (which can be retried)
/// and permanent failures.
#[derive(Debug, Clone)]
pub struct NonRetryableException {
    pub message: String,
    pub source: Option<String>,
}

impl NonRetryableException {
    /// Create a new `NonRetryableException` with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Create a new `NonRetryableException` with the given message and source error.
    pub fn with_source(message: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: Some(source.into()),
        }
    }
}

impl fmt::Display for NonRetryableException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(ref src) = self.source {
            write!(f, " (caused by: {})", src)?;
        }
        Ok(())
    }
}

impl std::error::Error for NonRetryableException {}
