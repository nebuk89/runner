// PromptManager mapping `PromptManager.cs`.
// Handles interactive and unattended prompts for runner configuration.

use anyhow::Result;
use std::io::{self, BufRead, Write};

/// Manages user prompts during configuration.
///
/// In unattended mode, prompts are not shown and defaults are used.
/// In interactive mode, the user is prompted via stdin/stdout.
pub struct PromptManager {
    unattended: bool,
}

impl PromptManager {
    /// Create a new `PromptManager`.
    ///
    /// If `unattended` is true, no interactive prompts are shown.
    pub fn new(unattended: bool) -> Self {
        Self { unattended }
    }

    /// Prompt for a required value (no default).
    ///
    /// In unattended mode, returns an error if the value is not provided
    /// via CLI args or environment variables.
    pub fn prompt_required(&self, prompt_text: &str) -> Result<String> {
        if self.unattended {
            return Err(anyhow::anyhow!(
                "Required input '{}' was not provided and running in unattended mode.",
                prompt_text
            ));
        }

        loop {
            print!("{}: ", prompt_text);
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().lock().read_line(&mut input)?;
            let trimmed = input.trim().to_string();

            if !trimmed.is_empty() {
                return Ok(trimmed);
            }

            println!("  (value is required)");
        }
    }

    /// Prompt for a value with a default.
    ///
    /// In unattended mode, the default is used without prompting.
    pub fn prompt_with_default(
        &self,
        prompt_text: &str,
        default: &str,
    ) -> Result<String> {
        if self.unattended {
            return Ok(default.to_string());
        }

        print!("{} [{}]: ", prompt_text, default);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let trimmed = input.trim().to_string();

        if trimmed.is_empty() {
            Ok(default.to_string())
        } else {
            Ok(trimmed)
        }
    }

    /// Prompt for a yes/no confirmation.
    ///
    /// In unattended mode, returns the `default_yes` value.
    pub fn prompt_yes_no(
        &self,
        prompt_text: &str,
        default_yes: bool,
    ) -> Result<bool> {
        if self.unattended {
            return Ok(default_yes);
        }

        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("{} {}: ", prompt_text, suffix);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();

        match trimmed.as_str() {
            "" => Ok(default_yes),
            "y" | "yes" => Ok(true),
            "n" | "no" => Ok(false),
            _ => Ok(default_yes),
        }
    }

    /// Prompt for a secret value (input is not echoed).
    ///
    /// In unattended mode, returns an error.
    pub fn prompt_secret(&self, prompt_text: &str) -> Result<String> {
        if self.unattended {
            return Err(anyhow::anyhow!(
                "Secret input '{}' was not provided and running in unattended mode.",
                prompt_text
            ));
        }

        print!("{}: ", prompt_text);
        io::stdout().flush()?;

        // Try to disable echo for password input
        #[cfg(unix)]
        {
            let stdin_handle = io::stdin();
            let original = nix::sys::termios::tcgetattr(&stdin_handle).ok();

            if let Some(ref orig) = original {
                let mut noecho = orig.clone();
                noecho.local_flags.remove(nix::sys::termios::LocalFlags::ECHO);
                let _ = nix::sys::termios::tcsetattr(
                    &stdin_handle,
                    nix::sys::termios::SetArg::TCSANOW,
                    &noecho,
                );
            }

            let mut input = String::new();
            stdin_handle.lock().read_line(&mut input)?;
            println!(); // Print newline since echo was disabled

            // Restore echo
            if let Some(ref orig) = original {
                let _ = nix::sys::termios::tcsetattr(
                    &stdin_handle,
                    nix::sys::termios::SetArg::TCSANOW,
                    orig,
                );
            }

            Ok(input.trim().to_string())
        }

        #[cfg(not(unix))]
        {
            let mut input = String::new();
            io::stdin().lock().read_line(&mut input)?;
            Ok(input.trim().to_string())
        }
    }

    /// Whether we are in unattended mode.
    pub fn is_unattended(&self) -> bool {
        self.unattended
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unattended_uses_default() {
        let pm = PromptManager::new(true);
        let result = pm.prompt_with_default("test", "default_val").unwrap();
        assert_eq!(result, "default_val");
    }

    #[test]
    fn test_unattended_required_fails() {
        let pm = PromptManager::new(true);
        let result = pm.prompt_required("test");
        assert!(result.is_err());
    }

    #[test]
    fn test_unattended_yes_no_default() {
        let pm = PromptManager::new(true);
        assert!(pm.prompt_yes_no("test", true).unwrap());
        assert!(!pm.prompt_yes_no("test", false).unwrap());
    }
}
