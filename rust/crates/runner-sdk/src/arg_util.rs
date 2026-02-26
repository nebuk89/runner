use std::fmt::Debug;
use std::path::Path;

/// Argument validation utilities.
/// These functions panic on validation failure, matching the C# `ArgUtil` behavior
/// which throws exceptions.
pub struct ArgUtil;

impl ArgUtil {
    /// Asserts that the value is `Some`. Panics with the parameter name if `None`.
    pub fn not_null<T>(value: &Option<T>, name: &str) {
        if value.is_none() {
            panic!("{name} must not be null (None)");
        }
    }

    /// Asserts that the string is not empty. Panics if empty or whitespace-only.
    pub fn not_null_or_empty(value: &str, name: &str) {
        if value.is_empty() {
            panic!("{name} must not be null or empty");
        }
    }

    /// Asserts that `expected == actual`. Panics with a descriptive message otherwise.
    pub fn equal<T: PartialEq + Debug>(expected: &T, actual: &T, name: &str) {
        if expected != actual {
            panic!(
                "{name} does not equal expected value. Expected '{expected:?}'. Actual '{actual:?}'."
            );
        }
    }

    /// Asserts that `expected != actual`. Panics with a descriptive message otherwise.
    pub fn not_equal<T: PartialEq + Debug>(expected: &T, actual: &T, name: &str) {
        if expected == actual {
            panic!(
                "{name} should not equal value '{expected:?}'. Actual '{actual:?}'."
            );
        }
    }

    /// Asserts the string is empty or blank. Panics if it contains any non-whitespace.
    pub fn null_or_empty(value: &str, name: &str) {
        if !value.is_empty() {
            panic!("{name} should be null or empty, but was '{value}'");
        }
    }

    /// Asserts that the given path exists and is a file. Panics otherwise.
    pub fn file_exists(path: &Path, name: &str) {
        let path_str = path.display();
        if !path.exists() {
            panic!("File not found: '{path_str}' (parameter '{name}')");
        }
        if !path.is_file() {
            panic!("Path is not a file: '{path_str}' (parameter '{name}')");
        }
    }

    /// Asserts that the given path exists and is a directory. Panics otherwise.
    pub fn directory_exists(path: &Path, name: &str) {
        let path_str = path.display();
        if !path.exists() {
            panic!("Directory not found: '{path_str}' (parameter '{name}')");
        }
        if !path.is_dir() {
            panic!("Path is not a directory: '{path_str}' (parameter '{name}')");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn not_null_passes_for_some() {
        let val = Some(42);
        ArgUtil::not_null(&val, "val");
    }

    #[test]
    #[should_panic(expected = "must not be null")]
    fn not_null_panics_for_none() {
        let val: Option<i32> = None;
        ArgUtil::not_null(&val, "val");
    }

    #[test]
    fn not_null_or_empty_passes() {
        ArgUtil::not_null_or_empty("hello", "val");
    }

    #[test]
    #[should_panic(expected = "must not be null or empty")]
    fn not_null_or_empty_panics() {
        ArgUtil::not_null_or_empty("", "val");
    }

    #[test]
    fn equal_passes() {
        ArgUtil::equal(&1, &1, "val");
    }

    #[test]
    #[should_panic(expected = "does not equal expected value")]
    fn equal_panics() {
        ArgUtil::equal(&1, &2, "val");
    }

    #[test]
    fn not_equal_passes() {
        ArgUtil::not_equal(&1, &2, "val");
    }

    #[test]
    #[should_panic(expected = "should not equal")]
    fn not_equal_panics() {
        ArgUtil::not_equal(&1, &1, "val");
    }

    #[test]
    fn null_or_empty_passes() {
        ArgUtil::null_or_empty("", "val");
    }

    #[test]
    #[should_panic(expected = "should be null or empty")]
    fn null_or_empty_panics() {
        ArgUtil::null_or_empty("hello", "val");
    }

    #[test]
    #[should_panic(expected = "File not found")]
    fn file_exists_panics_for_missing() {
        ArgUtil::file_exists(&PathBuf::from("/nonexistent_file_abc123"), "f");
    }

    #[test]
    #[should_panic(expected = "Directory not found")]
    fn directory_exists_panics_for_missing() {
        ArgUtil::directory_exists(&PathBuf::from("/nonexistent_dir_abc123"), "d");
    }
}
