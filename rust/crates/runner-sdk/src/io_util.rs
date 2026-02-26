use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::{fs, thread, time::Duration};

/// The executable file extension for the current platform.
#[cfg(target_os = "windows")]
pub const EXE_EXTENSION: &str = ".exe";
#[cfg(not(target_os = "windows"))]
pub const EXE_EXTENSION: &str = "";

/// The string comparison semantics for file paths on this platform.
/// On Linux, paths are case-sensitive (ordinal). On macOS and Windows, they are
/// case-insensitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePathComparison {
    CaseSensitive,
    CaseInsensitive,
}

#[cfg(target_os = "linux")]
pub const FILE_PATH_STRING_COMPARISON: FilePathComparison = FilePathComparison::CaseSensitive;
#[cfg(not(target_os = "linux"))]
pub const FILE_PATH_STRING_COMPARISON: FilePathComparison = FilePathComparison::CaseInsensitive;

/// I/O utility functions mapping `IOUtil.cs`.
pub struct IOUtil;

impl IOUtil {
    /// Recursively delete a directory with retry logic.
    ///
    /// If the initial removal fails (e.g. due to transient locks), the function
    /// retries up to 3 times with a small delay between attempts.
    pub fn delete_directory(path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        // If it's a symlink, just remove the link itself
        if path.symlink_metadata()?.file_type().is_symlink() {
            #[cfg(unix)]
            {
                fs::remove_file(path)
                    .with_context(|| format!("Failed to remove symlink '{}'", path.display()))?;
            }
            #[cfg(windows)]
            {
                // On Windows, symlinks to directories are removed with remove_dir
                if path.is_dir() {
                    fs::remove_dir(path).with_context(|| {
                        format!("Failed to remove directory symlink '{}'", path.display())
                    })?;
                } else {
                    fs::remove_file(path).with_context(|| {
                        format!("Failed to remove file symlink '{}'", path.display())
                    })?;
                }
            }
            return Ok(());
        }

        let max_retries = 3;
        let mut last_err = None;

        for attempt in 0..max_retries {
            // Try to remove read-only attributes on files before deletion
            if let Err(e) = Self::remove_readonly_recursive(path) {
                tracing::debug!(
                    "Failed to remove readonly attributes (attempt {}): {}",
                    attempt + 1,
                    e
                );
            }

            match fs::remove_dir_all(path) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_err = Some(e);
                    if attempt < max_retries - 1 {
                        thread::sleep(Duration::from_millis(100 * (attempt as u64 + 1)));
                    }
                }
            }
        }

        Err(last_err.unwrap()).with_context(|| {
            format!(
                "Failed to delete directory '{}' after {} retries",
                path.display(),
                max_retries
            )
        })
    }

    /// Delete a single file, removing the read-only attribute if necessary.
    pub fn delete_file(path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        // Attempt to remove read-only permission on the file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                let mode = perms.mode();
                if mode & 0o200 == 0 {
                    // Owner write bit not set; add it
                    perms.set_mode(mode | 0o200);
                    let _ = fs::set_permissions(path, perms);
                }
            }
        }

        #[cfg(windows)]
        {
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                if perms.readonly() {
                    perms.set_readonly(false);
                    let _ = fs::set_permissions(path, perms);
                }
            }
        }

        fs::remove_file(path)
            .with_context(|| format!("Failed to delete file '{}'", path.display()))?;
        Ok(())
    }

    /// Serialize a value as JSON and write it to a file.
    pub fn save_object<T: Serialize>(path: &Path, value: &T) -> Result<()> {
        let json = serde_json::to_string_pretty(value)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, json.as_bytes())
            .with_context(|| format!("Failed to write object to '{}'", path.display()))?;
        Ok(())
    }

    /// Read a file and deserialize it from JSON.
    pub fn load_object<T: DeserializeOwned>(path: &Path) -> Result<T> {
        let json = fs::read_to_string(path)
            .with_context(|| format!("Failed to read file '{}'", path.display()))?;
        let value = serde_json::from_str(&json)
            .with_context(|| format!("Failed to deserialize JSON from '{}'", path.display()))?;
        Ok(value)
    }

    /// Returns the directory containing the currently running executable.
    pub fn get_bin_path() -> PathBuf {
        std::env::current_exe()
            .expect("Failed to get current executable path")
            .parent()
            .expect("Executable has no parent directory")
            .to_path_buf()
    }

    /// Returns the "root" path, which is the parent of the bin directory.
    /// Typical layout: `<root>/bin/<exe>`
    pub fn get_root_path() -> PathBuf {
        Self::get_bin_path()
            .parent()
            .expect("Bin directory has no parent")
            .to_path_buf()
    }

    /// Returns the "externals" directory (sibling of bin).
    /// Typical layout: `<root>/externals/`
    pub fn get_extern_path() -> PathBuf {
        Self::get_root_path().join("externals")
    }

    /// Compare two path strings using the platform-appropriate case sensitivity.
    pub fn paths_equal(a: &str, b: &str) -> bool {
        match FILE_PATH_STRING_COMPARISON {
            FilePathComparison::CaseSensitive => a == b,
            FilePathComparison::CaseInsensitive => a.eq_ignore_ascii_case(b),
        }
    }

    /// Replace characters that are invalid in file names with `_`.
    pub fn replace_invalid_file_name_chars(file_name: &str) -> String {
        // Characters that are typically invalid in file names across platforms
        let invalid: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
        let mut result = String::with_capacity(file_name.len());
        for ch in file_name.chars() {
            if invalid.contains(&ch) || (ch as u32) < 0x20 {
                result.push('_');
            } else {
                result.push(ch);
            }
        }
        result
    }

    /// Recursively attempt to remove the read-only attribute from all items
    /// in a directory tree.
    fn remove_readonly_recursive(path: &Path) -> Result<()> {
        if path.is_file() {
            Self::remove_readonly(path)?;
            return Ok(());
        }

        if !path.is_dir() {
            return Ok(());
        }

        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            if entry_path.is_dir() {
                Self::remove_readonly_recursive(&entry_path)?;
            } else {
                Self::remove_readonly(&entry_path)?;
            }
        }
        Self::remove_readonly(path)?;
        Ok(())
    }

    /// Remove the read-only attribute from a single file-system entry.
    fn remove_readonly(path: &Path) -> Result<()> {
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            fs::set_permissions(path, perms)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Config {
        name: String,
        count: u32,
    }

    #[test]
    fn save_and_load_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let original = Config {
            name: "test".into(),
            count: 7,
        };
        IOUtil::save_object(&path, &original).unwrap();
        let loaded: Config = IOUtil::load_object(&path).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn delete_directory_missing_is_ok() {
        let result = IOUtil::delete_directory(Path::new("/tmp/nonexistent_runner_sdk_test_xyz"));
        assert!(result.is_ok());
    }

    #[test]
    fn delete_file_missing_is_ok() {
        let result = IOUtil::delete_file(Path::new("/tmp/nonexistent_runner_sdk_file_xyz"));
        assert!(result.is_ok());
    }

    #[test]
    fn delete_directory_works() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("subdir");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("file.txt"), b"data").unwrap();
        IOUtil::delete_directory(dir.path()).unwrap();
        assert!(!dir.path().exists());
    }

    #[test]
    fn delete_file_works() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, b"data").unwrap();
        IOUtil::delete_file(&file_path).unwrap();
        assert!(!file_path.exists());
    }

    #[test]
    fn get_bin_path_exists() {
        let bin = IOUtil::get_bin_path();
        assert!(bin.is_dir());
    }

    #[test]
    fn paths_equal_platform() {
        assert!(IOUtil::paths_equal("foo", "foo"));
        #[cfg(not(target_os = "linux"))]
        assert!(IOUtil::paths_equal("Foo", "foo"));
        #[cfg(target_os = "linux")]
        assert!(!IOUtil::paths_equal("Foo", "foo"));
    }

    #[test]
    fn replace_invalid_chars() {
        assert_eq!(
            IOUtil::replace_invalid_file_name_chars("a<b>c:d"),
            "a_b_c_d"
        );
    }

    #[test]
    fn exe_extension() {
        #[cfg(target_os = "windows")]
        assert_eq!(EXE_EXTENSION, ".exe");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(EXE_EXTENSION, "");
    }
}
