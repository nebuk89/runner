/// Build constants for the runner package.
/// In C# these are auto-generated at build time; here we use compile-time
/// environment variables with sensible defaults.

/// Source control information.
pub struct Source;

impl Source {
    /// The commit hash from which this binary was built.
    /// Set via the `RUNNER_COMMIT_HASH` env var at compile time, or "N/A".
    pub const COMMIT_HASH: &'static str = match option_env!("RUNNER_COMMIT_HASH") {
        Some(h) => h,
        None => "N/A",
    };
}

/// Runner package metadata.
#[derive(Debug, Clone)]
pub struct RunnerPackage;

impl RunnerPackage {
    /// The semantic version of the runner.
    /// Pulled from `CARGO_PKG_VERSION` which is set by Cargo from `Cargo.toml`.
    pub const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    /// The package / distribution name.
    /// Set via the `RUNNER_PACKAGE_NAME` env var at compile time, or "N/A".
    pub const PACKAGE_NAME: &'static str = match option_env!("RUNNER_PACKAGE_NAME") {
        Some(n) => n,
        None => "N/A",
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!RunnerPackage::VERSION.is_empty());
    }

    #[test]
    fn commit_hash_has_default() {
        // Will be "N/A" unless overridden at compile time
        assert!(!Source::COMMIT_HASH.is_empty());
    }

    #[test]
    fn package_name_has_default() {
        assert!(!RunnerPackage::PACKAGE_NAME.is_empty());
    }
}
