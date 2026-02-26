use crate::io_util::FILE_PATH_STRING_COMPARISON;
use crate::io_util::FilePathComparison;

/// PATH environment variable name (platform-specific).
///
/// On Windows the conventional name is `Path`; on Unix it is `PATH`.
#[cfg(target_os = "windows")]
pub const PATH_VARIABLE: &str = "Path";
#[cfg(not(target_os = "windows"))]
pub const PATH_VARIABLE: &str = "PATH";

/// Path utility functions mapping `PathUtil.cs`.
pub struct PathUtil;

impl PathUtil {
    /// Prepend `path` to the current process's PATH environment variable.
    ///
    /// If PATH is empty, sets it to just `path` (no trailing separator to
    /// avoid adding "current directory" to PATH on Unix).
    /// If `path` is already the first entry, the PATH is left unchanged.
    pub fn prepend_path(path: &str) {
        assert!(!path.is_empty(), "path must not be empty");

        let current = std::env::var(PATH_VARIABLE).unwrap_or_default();
        let separator = Self::path_separator();

        if current.is_empty() {
            std::env::set_var(PATH_VARIABLE, path);
            return;
        }

        // Check if the path is already the first entry
        let prefix = format!("{path}{separator}");
        let already_first = match FILE_PATH_STRING_COMPARISON {
            FilePathComparison::CaseSensitive => current.starts_with(&prefix),
            FilePathComparison::CaseInsensitive => current
                .to_lowercase()
                .starts_with(&prefix.to_lowercase()),
        };

        if already_first {
            return;
        }

        let new_path = format!("{path}{separator}{current}");
        std::env::set_var(PATH_VARIABLE, new_path);
    }

    /// Prepend `path` to the given `current_path` string and return the result.
    /// Does NOT modify the environment.
    pub fn prepend_path_value(path: &str, current_path: &str) -> String {
        assert!(!path.is_empty(), "path must not be empty");

        if current_path.is_empty() {
            return path.to_string();
        }

        let separator = Self::path_separator();
        let prefix = format!("{path}{separator}");

        let already_first = match FILE_PATH_STRING_COMPARISON {
            FilePathComparison::CaseSensitive => current_path.starts_with(&prefix),
            FilePathComparison::CaseInsensitive => current_path
                .to_lowercase()
                .starts_with(&prefix.to_lowercase()),
        };

        if already_first {
            return current_path.to_string();
        }

        format!("{path}{separator}{current_path}")
    }

    /// The platform-specific PATH entry separator character.
    fn path_separator() -> char {
        if cfg!(target_os = "windows") {
            ';'
        } else {
            ':'
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_variable_name() {
        #[cfg(target_os = "windows")]
        assert_eq!(PATH_VARIABLE, "Path");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(PATH_VARIABLE, "PATH");
    }

    #[test]
    fn prepend_path_value_empty() {
        assert_eq!(PathUtil::prepend_path_value("/usr/local/bin", ""), "/usr/local/bin");
    }

    #[test]
    fn prepend_path_value_non_empty() {
        let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
        let result = PathUtil::prepend_path_value("/new", &format!("/existing{sep}/other"));
        let expected = format!("/new{sep}/existing{sep}/other");
        assert_eq!(result, expected);
    }

    #[test]
    fn prepend_path_value_already_first() {
        let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
        let current = format!("/new{sep}/other");
        let result = PathUtil::prepend_path_value("/new", &current);
        assert_eq!(result, current);
    }
}
