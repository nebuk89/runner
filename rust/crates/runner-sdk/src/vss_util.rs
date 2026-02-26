use crate::string_util::StringUtil;
use crate::web_proxy::RunnerWebProxy;
use reqwest::Client;
use std::time::Duration;

/// VSS / HTTP client utility functions mapping `VssUtil.cs`.
///
/// Provides HTTP client creation with retry, timeout, proxy, and TLS
/// configuration driven by environment variables.
pub struct VssUtil;

impl VssUtil {
    /// The environment variable name for configuring HTTP retry count.
    pub const HTTP_RETRY_ENV: &'static str = "GITHUB_ACTIONS_RUNNER_HTTP_RETRY";
    /// The environment variable name for configuring HTTP timeout in seconds.
    pub const HTTP_TIMEOUT_ENV: &'static str = "GITHUB_ACTIONS_RUNNER_HTTP_TIMEOUT";
    /// The environment variable name for disabling TLS verification.
    pub const TLS_NO_VERIFY_ENV: &'static str = "GITHUB_ACTIONS_RUNNER_TLS_NO_VERIFY";

    /// Default retry count.
    pub const DEFAULT_RETRY: u32 = 3;
    /// Maximum allowed retry count.
    pub const MAX_RETRY: u32 = 10;
    /// Default timeout in seconds.
    pub const DEFAULT_TIMEOUT_SECS: u64 = 100;
    /// Maximum allowed timeout in seconds.
    pub const MAX_TIMEOUT_SECS: u64 = 1200;

    /// Read the configured retry count from the environment.
    ///
    /// Clamps the value to `[3, 10]`. Defaults to 3 if unset or invalid.
    pub fn get_retry_count() -> u32 {
        let raw = std::env::var(Self::HTTP_RETRY_ENV).unwrap_or_default();
        let parsed = raw.parse::<u32>().unwrap_or(Self::DEFAULT_RETRY);
        parsed.clamp(Self::DEFAULT_RETRY, Self::MAX_RETRY)
    }

    /// Read the configured timeout from the environment.
    ///
    /// Clamps the value to `[100, 1200]` seconds. Defaults to 100s if unset or invalid.
    pub fn get_timeout() -> Duration {
        let raw = std::env::var(Self::HTTP_TIMEOUT_ENV).unwrap_or_default();
        let secs = raw.parse::<u64>().unwrap_or(Self::DEFAULT_TIMEOUT_SECS);
        let clamped = secs.clamp(Self::DEFAULT_TIMEOUT_SECS, Self::MAX_TIMEOUT_SECS);
        Duration::from_secs(clamped)
    }

    /// Check whether TLS verification should be disabled.
    pub fn is_tls_no_verify() -> bool {
        let raw = std::env::var(Self::TLS_NO_VERIFY_ENV).unwrap_or_default();
        StringUtil::convert_to_bool(&raw) == Some(true)
    }

    /// Create a `reqwest::Client` configured with proxy, timeout, retry (via headers),
    /// and optional TLS verification bypass.
    ///
    /// Note: `reqwest` does not have built-in retry. The retry count is stored for
    /// higher-level retry loops to use. This function sets up the proxy and TLS config.
    pub fn create_http_client(proxy: &RunnerWebProxy) -> Client {
        let timeout = Self::get_timeout();
        let tls_no_verify = Self::is_tls_no_verify();

        let mut builder = Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(60))
            .danger_accept_invalid_certs(tls_no_verify)
            .user_agent(format!(
                "GitHubActionsRunner/{}",
                crate::build_constants::RunnerPackage::VERSION
            ));

        // Configure HTTP proxy
        if let Some(ref addr) = proxy.http_proxy_address {
            if let Ok(url) = addr.parse::<url::Url>() {
                let mut reqwest_proxy = reqwest::Proxy::http(url.as_str())
                    .expect("Failed to create HTTP proxy");

                if let (Some(ref user), Some(ref pass)) =
                    (&proxy.http_proxy_username, &proxy.http_proxy_password)
                {
                    reqwest_proxy = reqwest_proxy.basic_auth(user, pass);
                }

                builder = builder.proxy(reqwest_proxy);
            }
        }

        // Configure HTTPS proxy
        if let Some(ref addr) = proxy.https_proxy_address {
            if let Ok(url) = addr.parse::<url::Url>() {
                let mut reqwest_proxy = reqwest::Proxy::https(url.as_str())
                    .expect("Failed to create HTTPS proxy");

                if let (Some(ref user), Some(ref pass)) =
                    (&proxy.https_proxy_username, &proxy.https_proxy_password)
                {
                    reqwest_proxy = reqwest_proxy.basic_auth(user, pass);
                }

                builder = builder.proxy(reqwest_proxy);
            }
        }

        // Set up no_proxy rules via reqwest's built-in no_proxy
        if let Some(ref no_proxy_str) = proxy.no_proxy_string {
            // reqwest reads NO_PROXY from the environment, but we also set a custom
            // proxy with no_proxy support. We ensure the env var is set.
            std::env::set_var("NO_PROXY", no_proxy_str);
        }

        builder.build().expect("Failed to build HTTP client")
    }

    /// Create an HTTP client with default (environment-based) proxy settings.
    pub fn create_default_http_client() -> Client {
        let proxy = RunnerWebProxy::new();
        Self::create_http_client(&proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_env() {
        std::env::remove_var(VssUtil::HTTP_RETRY_ENV);
        std::env::remove_var(VssUtil::HTTP_TIMEOUT_ENV);
        std::env::remove_var(VssUtil::TLS_NO_VERIFY_ENV);
    }

    #[test]
    fn default_retry_count() {
        clear_env();
        assert_eq!(VssUtil::get_retry_count(), 3);
    }

    #[test]
    fn retry_count_clamped() {
        std::env::set_var(VssUtil::HTTP_RETRY_ENV, "1");
        assert_eq!(VssUtil::get_retry_count(), 3); // min is 3
        std::env::set_var(VssUtil::HTTP_RETRY_ENV, "20");
        assert_eq!(VssUtil::get_retry_count(), 10); // max is 10
        std::env::set_var(VssUtil::HTTP_RETRY_ENV, "7");
        assert_eq!(VssUtil::get_retry_count(), 7);
        clear_env();
    }

    #[test]
    fn default_timeout() {
        clear_env();
        assert_eq!(VssUtil::get_timeout(), Duration::from_secs(100));
    }

    #[test]
    fn timeout_clamped() {
        std::env::set_var(VssUtil::HTTP_TIMEOUT_ENV, "10");
        assert_eq!(VssUtil::get_timeout(), Duration::from_secs(100)); // min 100
        std::env::set_var(VssUtil::HTTP_TIMEOUT_ENV, "5000");
        assert_eq!(VssUtil::get_timeout(), Duration::from_secs(1200)); // max 1200
        std::env::set_var(VssUtil::HTTP_TIMEOUT_ENV, "500");
        assert_eq!(VssUtil::get_timeout(), Duration::from_secs(500));
        clear_env();
    }

    #[test]
    fn tls_no_verify_off_by_default() {
        clear_env();
        assert!(!VssUtil::is_tls_no_verify());
    }

    #[test]
    fn tls_no_verify_on() {
        std::env::set_var(VssUtil::TLS_NO_VERIFY_ENV, "true");
        assert!(VssUtil::is_tls_no_verify());
        clear_env();
    }

    #[test]
    fn create_client_succeeds() {
        clear_env();
        // Clear proxy env vars so we don't interfere
        for var in &["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY", "no_proxy", "NO_PROXY"] {
            std::env::remove_var(var);
        }
        let proxy = RunnerWebProxy::new();
        let _client = VssUtil::create_http_client(&proxy);
        // Just verify it builds without panicking
    }
}
