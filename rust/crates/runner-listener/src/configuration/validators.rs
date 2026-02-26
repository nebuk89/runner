// Validators mapping validation logic from the C# configuration classes.
// URL and runner name validation.

use anyhow::Result;
use url::Url;

/// Validate a GitHub URL.
///
/// The URL must be:
/// - A valid URL
/// - Using HTTP or HTTPS scheme
/// - Have a host
pub fn validate_url(url_str: &str) -> Result<()> {
    if url_str.is_empty() {
        return Err(anyhow::anyhow!("URL cannot be empty"));
    }

    let url = Url::parse(url_str).map_err(|e| {
        anyhow::anyhow!("Invalid URL '{}': {}", url_str, e)
    })?;

    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(anyhow::anyhow!(
                "URL must use HTTP or HTTPS scheme, got '{}'",
                scheme
            ));
        }
    }

    if url.host_str().is_none() {
        return Err(anyhow::anyhow!(
            "URL must have a host"
        ));
    }

    Ok(())
}

/// Validate a runner name.
///
/// The name must be:
/// - Non-empty
/// - At most 64 characters
/// - Contain only alphanumeric characters, hyphens, underscores, and periods
pub fn validate_runner_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow::anyhow!("Runner name cannot be empty"));
    }

    if name.len() > 64 {
        return Err(anyhow::anyhow!(
            "Runner name must be at most 64 characters (got {})",
            name.len()
        ));
    }

    // Allow alphanumeric, hyphens, underscores, and periods
    let is_valid = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.');

    if !is_valid {
        return Err(anyhow::anyhow!(
            "Runner name '{}' contains invalid characters. \
             Only alphanumeric characters, hyphens, underscores, and periods are allowed.",
            name
        ));
    }

    Ok(())
}

/// Validate a work folder path.
pub fn validate_work_folder(work: &str) -> Result<()> {
    if work.is_empty() {
        return Err(anyhow::anyhow!("Work folder cannot be empty"));
    }

    // Check for invalid characters
    let invalid = ['<', '>', ':', '"', '|', '?', '*'];
    for c in invalid {
        if work.contains(c) {
            return Err(anyhow::anyhow!(
                "Work folder contains invalid character: '{}'",
                c
            ));
        }
    }

    Ok(())
}

/// Validate comma-separated labels.
pub fn validate_labels(labels: &str) -> Result<()> {
    if labels.is_empty() {
        return Ok(());
    }

    for label in labels.split(',') {
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!(
                "Label list contains empty labels"
            ));
        }
        if trimmed.len() > 64 {
            return Err(anyhow::anyhow!(
                "Label '{}' exceeds the maximum length of 64 characters",
                trimmed
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_url() {
        assert!(validate_url("https://github.com/owner/repo").is_ok());
        assert!(validate_url("http://github.example.com/owner/repo").is_ok());
        assert!(validate_url("https://github.com").is_ok());
    }

    #[test]
    fn test_invalid_url() {
        assert!(validate_url("").is_err());
        assert!(validate_url("not-a-url").is_err());
        assert!(validate_url("ftp://github.com").is_err());
    }

    #[test]
    fn test_valid_runner_name() {
        assert!(validate_runner_name("my-runner").is_ok());
        assert!(validate_runner_name("runner_01").is_ok());
        assert!(validate_runner_name("runner.prod").is_ok());
        assert!(validate_runner_name("MyRunner123").is_ok());
    }

    #[test]
    fn test_invalid_runner_name() {
        assert!(validate_runner_name("").is_err());
        assert!(validate_runner_name("a".repeat(65).as_str()).is_err());
        assert!(validate_runner_name("runner name").is_err());
        assert!(validate_runner_name("runner@host").is_err());
    }

    #[test]
    fn test_validate_labels() {
        assert!(validate_labels("").is_ok());
        assert!(validate_labels("label1,label2").is_ok());
        assert!(validate_labels("label1, label2, label3").is_ok());
        assert!(validate_labels(",").is_err()); // empty label
    }

    #[test]
    fn test_validate_work_folder() {
        assert!(validate_work_folder("_work").is_ok());
        assert!(validate_work_folder("/tmp/work").is_ok());
        assert!(validate_work_folder("").is_err());
    }
}
