// Variables mapping `Variables.cs`.
// Thread-safe variable store with secret tracking, expansion, and environment block export.

use parking_lot::RwLock;
use runner_common::secret_masker::SecretMasker;
use std::collections::HashMap;
use std::sync::Arc;

use crate::worker::{AgentJobRequestMessage, VariableValueMessage};

/// A single variable entry with metadata.
#[derive(Debug, Clone)]
pub struct VariableValue {
    /// The variable value.
    pub value: String,
    /// Whether this variable is a secret (should be masked in output).
    pub is_secret: bool,
    /// Whether this variable is read-only (cannot be overwritten by the job).
    pub is_read_only: bool,
}

impl VariableValue {
    /// Create a new non-secret, writable variable.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            is_secret: false,
            is_read_only: false,
        }
    }

    /// Create a new secret variable.
    pub fn new_secret(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            is_secret: true,
            is_read_only: false,
        }
    }
}

/// Thread-safe variable store for job/step execution.
///
/// Variables are stored with case-insensitive keys (matching the C# behaviour).
/// Secret values are automatically registered with the `SecretMasker`.
#[derive(Clone)]
pub struct Variables {
    inner: Arc<RwLock<VariablesInner>>,
    secret_masker: Option<Arc<SecretMasker>>,
}

struct VariablesInner {
    /// Variables keyed by lowercase name.
    store: HashMap<String, VariableValue>,
    /// Maximum recursion depth for macro expansion.
    recurse_count: u32,
}

impl std::fmt::Debug for Variables {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read();
        f.debug_struct("Variables")
            .field("count", &inner.store.len())
            .finish()
    }
}

impl Variables {
    /// Create an empty variable store (no secret masker).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(VariablesInner {
                store: HashMap::new(),
                recurse_count: 0,
            })),
            secret_masker: None,
        }
    }

    /// Create a variable store with a secret masker.
    pub fn with_masker(masker: Arc<SecretMasker>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(VariablesInner {
                store: HashMap::new(),
                recurse_count: 0,
            })),
            secret_masker: Some(masker),
        }
    }

    /// Build a `Variables` instance from a job message, registering secrets with the masker.
    pub fn from_message(message: &AgentJobRequestMessage, masker: &Arc<SecretMasker>) -> Self {
        let vars = Self::with_masker(Arc::clone(masker));

        for (name, var) in &message.variables {
            let vv = VariableValue {
                value: var.value.clone(),
                is_secret: var.is_secret,
                is_read_only: var.is_read_only,
            };

            if var.is_secret && !var.value.is_empty() {
                masker.add_value(&var.value);
            }

            vars.inner.write().store.insert(name.to_lowercase(), vv);
        }

        vars
    }

    /// Get a variable value by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<String> {
        let inner = self.inner.read();
        inner.store.get(&name.to_lowercase()).map(|v| v.value.clone())
    }

    /// Try to get the full `VariableValue` (including metadata) by name.
    pub fn try_get_value(&self, name: &str) -> Option<VariableValue> {
        let inner = self.inner.read();
        inner.store.get(&name.to_lowercase()).cloned()
    }

    /// Set a variable. If it's marked as secret, registers it with the masker.
    /// Returns `false` if the variable is read-only and was not overwritten.
    pub fn set(&self, name: &str, value: impl Into<String>, is_secret: bool) -> bool {
        let value = value.into();
        let key = name.to_lowercase();
        let mut inner = self.inner.write();

        // Check if read-only
        if let Some(existing) = inner.store.get(&key) {
            if existing.is_read_only {
                return false;
            }
        }

        if is_secret {
            if let Some(ref masker) = self.secret_masker {
                if !value.is_empty() {
                    masker.add_value(&value);
                }
            }
        }

        inner.store.insert(
            key,
            VariableValue {
                value,
                is_secret,
                is_read_only: false,
            },
        );

        true
    }

    /// Set a variable as read-only.
    pub fn set_read_only(&self, name: &str, value: impl Into<String>, is_secret: bool) {
        let value = value.into();
        let key = name.to_lowercase();

        if is_secret {
            if let Some(ref masker) = self.secret_masker {
                if !value.is_empty() {
                    masker.add_value(&value);
                }
            }
        }

        let mut inner = self.inner.write();
        inner.store.insert(
            key,
            VariableValue {
                value,
                is_secret,
                is_read_only: true,
            },
        );
    }

    /// Remove a variable by name.
    pub fn remove(&self, name: &str) {
        let mut inner = self.inner.write();
        inner.store.remove(&name.to_lowercase());
    }

    /// Check if a variable exists.
    pub fn contains_key(&self, name: &str) -> bool {
        let inner = self.inner.read();
        inner.store.contains_key(&name.to_lowercase())
    }

    /// Get the number of variables.
    pub fn len(&self) -> usize {
        self.inner.read().store.len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.read().store.is_empty()
    }

    /// Get the macro expansion recursion count.
    pub fn recurse_count(&self) -> u32 {
        self.inner.read().recurse_count
    }

    /// Increment and return the recursion count (for macro expansion).
    pub fn increment_recurse_count(&self) -> u32 {
        let mut inner = self.inner.write();
        inner.recurse_count += 1;
        inner.recurse_count
    }

    /// Reset the recursion count.
    pub fn reset_recurse_count(&self) {
        self.inner.write().recurse_count = 0;
    }

    /// Expand `$(variable)` macros in a string using the current variable store.
    /// Supports recursive expansion up to a maximum depth.
    pub fn expand_values(&self, input: &str) -> String {
        const MAX_RECURSE: u32 = 50;

        let mut result = input.to_string();
        self.reset_recurse_count();

        loop {
            let depth = self.increment_recurse_count();
            if depth > MAX_RECURSE {
                break;
            }

            let mut replaced = false;
            let inner = self.inner.read();

            for (name, var) in &inner.store {
                let macro_token = format!("$({name})");
                if result.contains(&macro_token) {
                    result = result.replace(&macro_token, &var.value);
                    replaced = true;
                }
            }

            if !replaced {
                break;
            }
        }

        result
    }

    /// Copy all non-secret variables into an environment block.
    pub fn copy_into_env_block(&self) -> HashMap<String, String> {
        let inner = self.inner.read();
        let mut env = HashMap::new();
        for (name, var) in &inner.store {
            // Skip internal variables (those with dots)
            if name.contains('.') {
                continue;
            }
            env.insert(name.clone(), var.value.clone());
        }
        env
    }

    /// Get all variable names.
    pub fn keys(&self) -> Vec<String> {
        self.inner.read().store.keys().cloned().collect()
    }

    /// Get all variables as a snapshot.
    pub fn snapshot(&self) -> HashMap<String, VariableValue> {
        self.inner.read().store.clone()
    }

    /// Merge variables from another Variables instance (non-read-only values only).
    pub fn merge_from(&self, other: &Variables) {
        let other_snap = other.snapshot();
        for (name, var) in other_snap {
            if !var.is_read_only {
                self.set(&name, var.value, var.is_secret);
            }
        }
    }
}

impl Default for Variables {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_set() {
        let vars = Variables::new();
        vars.set("MY_VAR", "hello", false);
        assert_eq!(vars.get("MY_VAR"), Some("hello".to_string()));
        assert_eq!(vars.get("my_var"), Some("hello".to_string()));
    }

    #[test]
    fn test_case_insensitive() {
        let vars = Variables::new();
        vars.set("FooBar", "value1", false);
        assert_eq!(vars.get("foobar"), Some("value1".to_string()));
        assert_eq!(vars.get("FOOBAR"), Some("value1".to_string()));
    }

    #[test]
    fn test_read_only() {
        let vars = Variables::new();
        vars.set_read_only("TOKEN", "secret123", true);
        let ok = vars.set("TOKEN", "overwrite", false);
        assert!(!ok);
        assert_eq!(vars.get("TOKEN"), Some("secret123".to_string()));
    }

    #[test]
    fn test_secret_masker_integration() {
        let masker = Arc::new(SecretMasker::new());
        let vars = Variables::with_masker(masker.clone());
        vars.set("SECRET_KEY", "my-api-key", true);
        assert_eq!(masker.mask_secrets("token is my-api-key"), "token is ***");
    }

    #[test]
    fn test_expand_values() {
        let vars = Variables::new();
        vars.set("greeting", "hello", false);
        vars.set("target", "world", false);
        let result = vars.expand_values("$(greeting) $(target)!");
        assert_eq!(result, "hello world!");
    }

    #[test]
    fn test_copy_into_env_block() {
        let vars = Variables::new();
        vars.set("PUBLIC_VAR", "value1", false);
        vars.set("system.internal", "value2", false);
        let env = vars.copy_into_env_block();
        assert!(env.contains_key("public_var"));
        assert!(!env.contains_key("system.internal"));
    }

    #[test]
    fn test_from_message() {
        let masker = Arc::new(SecretMasker::new());
        let mut message = crate::worker::AgentJobRequestMessage {
            job_id: String::new(),
            job_display_name: String::new(),
            request_id: 0,
            plan_id: String::new(),
            timeline_id: String::new(),
            environment_variables: HashMap::new(),
            variables: HashMap::new(),
            steps: Vec::new(),
            resources: Default::default(),
            workspace: None,
            file_table: Vec::new(),
            context_data: HashMap::new(),
            job_container: None,
            job_service_containers: Vec::new(),
            actor: String::new(),
            message_type: String::new(),
        };

        message.variables.insert(
            "MY_SECRET".to_string(),
            VariableValueMessage {
                value: "password123".to_string(),
                is_secret: true,
                is_read_only: false,
            },
        );

        let vars = Variables::from_message(&message, &masker);
        assert_eq!(vars.get("MY_SECRET"), Some("password123".to_string()));
        assert_eq!(masker.mask_secrets("password123"), "***");
    }
}
