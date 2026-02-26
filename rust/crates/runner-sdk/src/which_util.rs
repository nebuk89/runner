use std::path::Path;

/// Which-utility for locating executables on PATH.
/// Maps `WhichUtil.cs`.
pub struct WhichUtil;

impl WhichUtil {
    /// Locate the first occurrence of `command` on the system PATH.
    ///
    /// - On Unix, checks that the candidate has execute permission.
    /// - On Windows, considers PATHEXT extensions.
    /// - If `require` is true and the command is not found, returns an error.
    /// - If `require` is false and the command is not found, returns `Ok(None)`.
    pub fn which(command: &str, require: bool) -> anyhow::Result<Option<std::path::PathBuf>> {
        if command.is_empty() {
            if require {
                anyhow::bail!("command must not be empty");
            }
            return Ok(None);
        }

        // If the command is already a fully-qualified path that exists, return it
        let command_path = Path::new(command);
        if command_path.is_absolute() && command_path.is_file() {
            if Self::is_executable(command_path) {
                return Ok(Some(command_path.to_path_buf()));
            }
        }

        let path_var = std::env::var("PATH").unwrap_or_default();
        if path_var.is_empty() {
            if require {
                anyhow::bail!("{command}: command not found. PATH is not defined.");
            }
            return Ok(None);
        }

        let path_segments: Vec<&str> = path_var.split(Self::path_separator()).collect();

        for segment in &path_segments {
            if segment.is_empty() {
                continue;
            }
            let dir = Path::new(segment);
            if !dir.is_dir() {
                continue;
            }

            // On Windows, try PATHEXT extensions
            #[cfg(target_os = "windows")]
            {
                if let Some(found) = Self::find_with_pathext(dir, command) {
                    return Ok(Some(found));
                }
            }

            // On Unix (or Windows fallback for already-extensioned commands)
            #[cfg(not(target_os = "windows"))]
            {
                let candidate = dir.join(command);
                if candidate.is_file() && Self::is_executable(&candidate) {
                    return Ok(Some(candidate));
                }
            }
        }

        if require {
            anyhow::bail!(
                "{command}: command not found. Make sure '{command}' is installed and its location included in the 'PATH' environment variable."
            );
        }
        Ok(None)
    }

    /// Find all occurrences of `command` on the system PATH.
    pub fn which_all(command: &str) -> Vec<std::path::PathBuf> {
        if command.is_empty() {
            return Vec::new();
        }

        let path_var = std::env::var("PATH").unwrap_or_default();
        let path_segments: Vec<&str> = path_var.split(Self::path_separator()).collect();
        let mut results = Vec::new();

        for segment in &path_segments {
            if segment.is_empty() {
                continue;
            }
            let dir = Path::new(segment);
            if !dir.is_dir() {
                continue;
            }

            #[cfg(target_os = "windows")]
            {
                if let Some(found) = Self::find_with_pathext(dir, command) {
                    results.push(found);
                }
            }

            #[cfg(not(target_os = "windows"))]
            {
                let candidate = dir.join(command);
                if candidate.is_file() && Self::is_executable(&candidate) {
                    results.push(candidate);
                }
            }
        }

        results
    }

    /// Returns the PATH separator for the current platform.
    fn path_separator() -> char {
        if cfg!(target_os = "windows") {
            ';'
        } else {
            ':'
        }
    }

    /// Check if a file has the execute permission.
    #[cfg(unix)]
    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }

    #[cfg(not(unix))]
    fn is_executable(path: &Path) -> bool {
        // On Windows, any existing file is considered "executable" if its extension
        // is in PATHEXT (handled by find_with_pathext). For a direct check, just
        // verify it exists.
        path.is_file()
    }

    /// On Windows, search for the command considering PATHEXT extensions.
    #[cfg(target_os = "windows")]
    fn find_with_pathext(dir: &Path, command: &str) -> Option<std::path::PathBuf> {
        let pathext = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH".to_string());

        let extensions: Vec<&str> = pathext.split(';').filter(|s| !s.is_empty()).collect();

        // Check if command already has a known extension
        let has_ext = extensions.iter().any(|ext| {
            command
                .to_lowercase()
                .ends_with(&ext.to_lowercase())
        });

        if has_ext {
            let candidate = dir.join(command);
            if candidate.is_file() {
                return Some(candidate);
            }
        } else {
            // Try each PATHEXT extension
            for ext in &extensions {
                let name = format!("{command}{ext}");
                let candidate = dir.join(&name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_common_command() {
        // On any Unix system, "sh" should be found
        #[cfg(unix)]
        {
            let result = WhichUtil::which("sh", false).unwrap();
            assert!(result.is_some());
            let path = result.unwrap();
            assert!(path.is_file());
        }
    }

    #[test]
    fn which_returns_none_for_missing() {
        let result = WhichUtil::which("nonexistent_command_xyz_123", false).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn which_errors_when_required_and_missing() {
        let result = WhichUtil::which("nonexistent_command_xyz_123", true);
        assert!(result.is_err());
    }

    #[test]
    fn which_all_finds_commands() {
        #[cfg(unix)]
        {
            let results = WhichUtil::which_all("sh");
            assert!(!results.is_empty());
        }
    }

    #[test]
    fn which_all_empty_for_missing() {
        let results = WhichUtil::which_all("nonexistent_command_xyz_123");
        assert!(results.is_empty());
    }
}
