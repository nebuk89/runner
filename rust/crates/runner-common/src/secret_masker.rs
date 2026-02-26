// SecretMasker mapping the C# `ISecretMasker` / `SecretMasker`.
// Provides a thread-safe store of secret values and replaces them in output strings.

use parking_lot::RwLock;
use std::sync::Arc;

/// Replacement text used when a secret is found.
const MASK: &str = "***";

/// A thread-safe secret masker that replaces registered secret values
/// in arbitrary strings with `***`.
#[derive(Debug, Clone)]
pub struct SecretMasker {
    inner: Arc<RwLock<SecretMaskerInner>>,
}

#[derive(Debug)]
struct SecretMaskerInner {
    /// All registered secret values (plain text).
    secrets: Vec<String>,
    /// Minimum length of a secret to be considered for masking.
    min_secret_length: usize,
}

impl Default for SecretMasker {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretMasker {
    /// Create a new empty `SecretMasker`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(SecretMaskerInner {
                secrets: Vec::new(),
                min_secret_length: 0,
            })),
        }
    }

    /// Register a new secret value that should be masked in output.
    /// Empty or whitespace-only values are ignored.
    pub fn add_value(&self, secret: &str) {
        let trimmed = secret.trim();
        if trimmed.is_empty() {
            return;
        }

        let mut inner = self.inner.write();
        // Avoid duplicates
        if !inner.secrets.iter().any(|s| s == trimmed) {
            inner.secrets.push(trimmed.to_string());
            // Re-sort by length descending so longer secrets are matched first
            // (prevents partial masking when one secret is a substring of another).
            inner.secrets.sort_by(|a, b| b.len().cmp(&a.len()));
            // Update min length
            inner.min_secret_length = inner.secrets.iter().map(|s| s.len()).min().unwrap_or(0);
        }
    }

    /// Remove all registered secrets.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.secrets.clear();
        inner.min_secret_length = 0;
    }

    /// Replace all registered secret values in `input` with `***`.
    ///
    /// Performs a simple iterative replacement. Longer secrets are replaced
    /// first to avoid partial matches.
    pub fn mask_secrets(&self, input: &str) -> String {
        let inner = self.inner.read();

        if inner.secrets.is_empty() || input.len() < inner.min_secret_length {
            return input.to_string();
        }

        let mut result = input.to_string();
        for secret in &inner.secrets {
            if result.contains(secret.as_str()) {
                result = result.replace(secret.as_str(), MASK);
            }
        }

        result
    }

    /// Returns the number of registered secrets.
    pub fn secret_count(&self) -> usize {
        self.inner.read().secrets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_single_secret() {
        let masker = SecretMasker::new();
        masker.add_value("password123");
        assert_eq!(masker.mask_secrets("my password123 is here"), "my *** is here");
    }

    #[test]
    fn test_mask_multiple_secrets() {
        let masker = SecretMasker::new();
        masker.add_value("secret1");
        masker.add_value("secret2");
        let result = masker.mask_secrets("secret1 and secret2 values");
        assert_eq!(result, "*** and *** values");
    }

    #[test]
    fn test_mask_overlapping_secrets() {
        let masker = SecretMasker::new();
        masker.add_value("pass");
        masker.add_value("password");
        // "password" should be replaced first because it's longer
        let result = masker.mask_secrets("my password is here");
        assert_eq!(result, "my *** is here");
    }

    #[test]
    fn test_empty_secret_ignored() {
        let masker = SecretMasker::new();
        masker.add_value("");
        masker.add_value("   ");
        assert_eq!(masker.secret_count(), 0);
    }

    #[test]
    fn test_no_secrets_passthrough() {
        let masker = SecretMasker::new();
        assert_eq!(masker.mask_secrets("hello world"), "hello world");
    }
}
