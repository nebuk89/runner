// VarUtil mapping `Util/VarUtil.cs`.
// Platform-aware environment variable helpers.

use crate::constants::{CURRENT_ARCHITECTURE, CURRENT_PLATFORM, Architecture, OsPlatform};

/// Platform-aware environment variable and OS helpers.
pub struct VarUtil;

impl VarUtil {
    /// Returns a string comparer function appropriate for environment variable
    /// key comparison on the current platform.
    ///
    /// - Linux/macOS: case-sensitive (ordinal)
    /// - Windows: case-insensitive
    pub fn env_var_keys_equal(a: &str, b: &str) -> bool {
        match CURRENT_PLATFORM {
            OsPlatform::Windows => a.eq_ignore_ascii_case(b),
            _ => a == b,
        }
    }

    /// Returns the OS name as a display string.
    pub fn os() -> &'static str {
        match CURRENT_PLATFORM {
            OsPlatform::Linux => "Linux",
            OsPlatform::MacOS => "macOS",
            OsPlatform::Windows => "Windows",
        }
    }

    /// Returns the CPU architecture as a display string.
    pub fn os_architecture() -> &'static str {
        match CURRENT_ARCHITECTURE {
            Architecture::X86 => "X86",
            Architecture::X64 => "X64",
            Architecture::Arm => "ARM",
            Architecture::Arm64 => "ARM64",
        }
    }

    /// Merge environment variable maps with platform-appropriate key comparison.
    ///
    /// Values from `overrides` take precedence over `base`.
    pub fn merge_env(
        base: &std::collections::HashMap<String, String>,
        overrides: &std::collections::HashMap<String, String>,
    ) -> std::collections::HashMap<String, String> {
        let mut merged = base.clone();
        for (key, value) in overrides {
            // On Windows, we need case-insensitive key matching
            if CURRENT_PLATFORM == OsPlatform::Windows {
                // Remove any existing key that matches case-insensitively
                let existing_key = merged
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(key))
                    .cloned();
                if let Some(k) = existing_key {
                    merged.remove(&k);
                }
            }
            merged.insert(key.clone(), value.clone());
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os_name() {
        let os = VarUtil::os();
        assert!(!os.is_empty());
        // Should be one of the known OS names
        assert!(
            os == "Linux" || os == "macOS" || os == "Windows",
            "Unexpected OS name: {}",
            os
        );
    }

    #[test]
    fn test_os_architecture() {
        let arch = VarUtil::os_architecture();
        assert!(!arch.is_empty());
        assert!(
            arch == "X86" || arch == "X64" || arch == "ARM" || arch == "ARM64",
            "Unexpected architecture: {}",
            arch
        );
    }

    #[test]
    fn test_env_var_comparison() {
        // On all platforms, exact match should always work
        assert!(VarUtil::env_var_keys_equal("PATH", "PATH"));
        assert!(!VarUtil::env_var_keys_equal("PATH", "path_other"));
    }
}
