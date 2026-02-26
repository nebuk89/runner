// CommandSettings mapping `CommandSettings.cs`.
// Parses CLI arguments and flags, with env var fallback (ACTIONS_RUNNER_INPUT_*).

use std::collections::HashMap;
use std::env;

use runner_common::constants::command_line;

/// Environment variable prefix for runner input overrides.
const ENV_PREFIX: &str = "ACTIONS_RUNNER_INPUT_";

/// Parsed command settings from CLI arguments and environment variables.
///
/// Maps `CommandSettings` in the C# runner. Supports named arguments (`--key value`),
/// boolean flags (`--flag`), and a single top-level command (`configure`, `remove`, `run`, etc.).
#[derive(Debug, Clone)]
pub struct CommandSettings {
    /// The top-level command (e.g. "configure", "remove", "run").
    command: Option<String>,
    /// Named arguments (key=value).
    args: HashMap<String, String>,
    /// Boolean flags.
    flags: HashMap<String, bool>,
    /// Raw arguments from the command line.
    #[allow(dead_code)]
    raw_args: Vec<String>,
}

impl CommandSettings {
    /// Parse command settings from the process command-line arguments.
    pub fn parse() -> Self {
        let raw_args: Vec<String> = env::args().skip(1).collect();
        Self::parse_from(&raw_args)
    }

    /// Parse command settings from the given argument list (for testing).
    pub fn parse_from(args: &[String]) -> Self {
        let mut command = None;
        let mut named_args = HashMap::new();
        let mut flags = HashMap::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];

            if arg.starts_with("--") {
                let key = arg.trim_start_matches("--").to_lowercase();

                // Check if this is a known named argument that takes a value
                if is_named_arg(&key) && i + 1 < args.len() {
                    let value = args[i + 1].clone();
                    named_args.insert(key, value);
                    i += 2;
                } else {
                    // Treat as boolean flag
                    flags.insert(key, true);
                    i += 1;
                }
            } else if command.is_none() && !arg.starts_with('-') {
                // First non-flag argument is the command
                command = Some(arg.to_lowercase());
                i += 1;
            } else {
                i += 1;
            }
        }

        Self {
            command,
            args: named_args,
            flags,
            raw_args: args.to_vec(),
        }
    }

    // -----------------------------------------------------------------------
    // Command detection
    // -----------------------------------------------------------------------

    /// Returns the top-level command, if any.
    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Whether the "configure" command was specified.
    pub fn is_configure(&self) -> bool {
        self.command.as_deref() == Some(command_line::commands::CONFIGURE)
    }

    /// Whether the "remove" command was specified.
    pub fn is_remove(&self) -> bool {
        self.command.as_deref() == Some(command_line::commands::REMOVE)
    }

    /// Whether the "run" command was specified (or no command at all — the default).
    pub fn is_run(&self) -> bool {
        matches!(self.command.as_deref(), None | Some("run"))
    }

    /// Whether the "warmup" command was specified.
    pub fn is_warmup(&self) -> bool {
        self.command.as_deref() == Some(command_line::commands::WARMUP)
    }

    // -----------------------------------------------------------------------
    // Named argument accessors
    // -----------------------------------------------------------------------

    /// Get a named argument value, falling back to the ACTIONS_RUNNER_INPUT_* env var.
    pub fn get_arg(&self, name: &str) -> Option<String> {
        let key = name.to_lowercase();

        // First check CLI args
        if let Some(val) = self.args.get(&key) {
            return Some(val.clone());
        }

        // Fallback to environment variable
        let env_key = format!("{}{}", ENV_PREFIX, name.to_uppercase());
        if let Ok(val) = env::var(&env_key) {
            if !val.is_empty() {
                return Some(val);
            }
        }

        None
    }

    /// Get the URL argument.
    pub fn get_url(&self) -> Option<String> {
        self.get_arg(command_line::args::URL)
    }

    /// Get the auth type argument.
    pub fn get_auth(&self) -> Option<String> {
        self.get_arg(command_line::args::AUTH)
    }

    /// Get the token argument.
    pub fn get_token(&self) -> Option<String> {
        self.get_arg(command_line::args::TOKEN)
    }

    /// Get the PAT argument.
    pub fn get_pat(&self) -> Option<String> {
        self.get_arg(command_line::args::PAT)
    }

    /// Get the runner name argument.
    pub fn get_name(&self) -> Option<String> {
        self.get_arg(command_line::args::NAME)
    }

    /// Get the work directory argument.
    pub fn get_work(&self) -> Option<String> {
        self.get_arg(command_line::args::WORK)
    }

    /// Get the labels argument.
    pub fn get_labels(&self) -> Option<String> {
        self.get_arg(command_line::args::LABELS)
    }

    /// Get the runner group argument.
    pub fn get_runner_group(&self) -> Option<String> {
        self.get_arg(command_line::args::RUNNER_GROUP)
    }

    /// Get the monitor socket address argument.
    pub fn get_monitor_socket_address(&self) -> Option<String> {
        self.get_arg(command_line::args::MONITOR_SOCKET_ADDRESS)
    }

    /// Get the startup type argument.
    pub fn get_startup_type(&self) -> Option<String> {
        self.get_arg(command_line::args::STARTUP_TYPE)
    }

    /// Get the username argument.
    pub fn get_user_name(&self) -> Option<String> {
        self.get_arg(command_line::args::USER_NAME)
    }

    /// Get the JIT config argument.
    pub fn get_jit_config(&self) -> Option<String> {
        self.get_arg(command_line::args::JIT_CONFIG)
    }

    /// Get the Windows logon account argument.
    pub fn get_windows_logon_account(&self) -> Option<String> {
        self.get_arg(command_line::args::WINDOWS_LOGON_ACCOUNT)
    }

    /// Get the Windows logon password argument.
    pub fn get_windows_logon_password(&self) -> Option<String> {
        self.get_arg(command_line::args::WINDOWS_LOGON_PASSWORD)
    }

    // -----------------------------------------------------------------------
    // Flag accessors
    // -----------------------------------------------------------------------

    /// Get a flag value, falling back to the ACTIONS_RUNNER_INPUT_* env var.
    pub fn get_flag(&self, name: &str) -> bool {
        let key = name.to_lowercase();

        // CLI flag
        if self.flags.get(&key) == Some(&true) {
            return true;
        }

        // Fallback to environment variable
        let env_key = format!("{}{}", ENV_PREFIX, name.to_uppercase());
        if let Ok(val) = env::var(&env_key) {
            if let Some(b) = runner_sdk::StringUtil::convert_to_bool(&val) {
                return b;
            }
        }

        false
    }

    /// Whether the --check flag is set.
    pub fn is_check(&self) -> bool {
        self.get_flag(command_line::flags::CHECK)
    }

    /// Whether the --commit flag is set.
    pub fn is_commit(&self) -> bool {
        self.get_flag(command_line::flags::COMMIT)
    }

    /// Whether the --ephemeral flag is set.
    pub fn is_ephemeral(&self) -> bool {
        self.get_flag(command_line::flags::EPHEMERAL)
    }

    /// Whether the --generateServiceConfig flag is set.
    pub fn is_generate_service_config(&self) -> bool {
        self.get_flag(command_line::flags::GENERATE_SERVICE_CONFIG)
    }

    /// Whether the --help flag is set.
    pub fn is_help(&self) -> bool {
        self.get_flag(command_line::flags::HELP)
    }

    /// Whether the --local flag is set.
    pub fn is_local(&self) -> bool {
        self.get_flag(command_line::flags::LOCAL)
    }

    /// Whether the --no-default-labels flag is set.
    pub fn is_no_default_labels(&self) -> bool {
        self.get_flag(command_line::flags::NO_DEFAULT_LABELS)
    }

    /// Whether the --replace flag is set.
    pub fn is_replace(&self) -> bool {
        self.get_flag(command_line::flags::REPLACE)
    }

    /// Whether the --disableupdate flag is set.
    pub fn is_disable_update(&self) -> bool {
        self.get_flag(command_line::flags::DISABLE_UPDATE)
    }

    /// Whether the --once flag is set.
    pub fn is_once(&self) -> bool {
        self.get_flag(command_line::flags::ONCE)
    }

    /// Whether the --runasservice flag is set.
    pub fn is_run_as_service(&self) -> bool {
        self.get_flag(command_line::flags::RUN_AS_SERVICE)
    }

    /// Whether the --unattended flag is set.
    pub fn is_unattended(&self) -> bool {
        self.get_flag(command_line::flags::UNATTENDED)
    }

    /// Whether the --version flag is set.
    pub fn is_version(&self) -> bool {
        self.get_flag(command_line::flags::VERSION)
    }

    /// Returns a list of argument names that contain secret values (should be masked).
    pub fn secret_arg_names() -> &'static [&'static str] {
        command_line::args::secrets()
    }

    /// Get all named arguments (for inspection/logging without secrets).
    pub fn sanitized_args(&self) -> HashMap<String, String> {
        let secrets = Self::secret_arg_names();
        self.args
            .iter()
            .map(|(k, v)| {
                if secrets.iter().any(|s| s.eq_ignore_ascii_case(k)) {
                    (k.clone(), "***".to_string())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect()
    }
}

/// Whether the given key is a named argument that takes a value (not a boolean flag).
fn is_named_arg(key: &str) -> bool {
    matches!(
        key,
        "auth"
            | "labels"
            | "monitorsocketaddress"
            | "name"
            | "runnergroup"
            | "startuptype"
            | "url"
            | "username"
            | "windowslogonaccount"
            | "work"
            | "token"
            | "pat"
            | "windowslogonpassword"
            | "jitconfig"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_configure_command() {
        let args = vec![
            "configure".to_string(),
            "--url".to_string(),
            "https://github.com/owner/repo".to_string(),
            "--token".to_string(),
            "ABCDEF".to_string(),
        ];
        let settings = CommandSettings::parse_from(&args);
        assert!(settings.is_configure());
        assert_eq!(
            settings.get_url().unwrap(),
            "https://github.com/owner/repo"
        );
        assert_eq!(settings.get_token().unwrap(), "ABCDEF");
    }

    #[test]
    fn test_parse_run_command_with_flags() {
        let args = vec![
            "run".to_string(),
            "--once".to_string(),
            "--ephemeral".to_string(),
        ];
        let settings = CommandSettings::parse_from(&args);
        assert!(settings.is_run());
        assert!(settings.is_once());
        assert!(settings.is_ephemeral());
        assert!(!settings.is_help());
    }

    #[test]
    fn test_no_command_defaults_to_run() {
        let args: Vec<String> = vec![];
        let settings = CommandSettings::parse_from(&args);
        assert!(settings.is_run());
    }

    #[test]
    fn test_sanitized_args_masks_secrets() {
        let args = vec![
            "configure".to_string(),
            "--url".to_string(),
            "https://github.com".to_string(),
            "--token".to_string(),
            "my-secret-token".to_string(),
        ];
        let settings = CommandSettings::parse_from(&args);
        let sanitized = settings.sanitized_args();
        assert_eq!(sanitized.get("token").unwrap(), "***");
        assert_eq!(sanitized.get("url").unwrap(), "https://github.com");
    }

    #[test]
    fn test_version_flag() {
        let args = vec!["--version".to_string()];
        let settings = CommandSettings::parse_from(&args);
        assert!(settings.is_version());
    }
}
