use once_cell::sync::Lazy;
use regex::Regex;
use url::Url;

/// Information about a proxy bypass entry.
#[derive(Debug, Clone)]
pub struct ByPassInfo {
    pub host: String,
    pub port: Option<String>,
}

/// Runner web proxy configuration.
///
/// Reads `http_proxy` / `HTTP_PROXY`, `https_proxy` / `HTTPS_PROXY`,
/// and `no_proxy` / `NO_PROXY` environment variables to configure proxy routing.
///
/// Maps `RunnerWebProxy.cs`.
#[derive(Debug, Clone)]
pub struct RunnerWebProxy {
    pub http_proxy_address: Option<String>,
    pub http_proxy_username: Option<String>,
    pub http_proxy_password: Option<String>,

    pub https_proxy_address: Option<String>,
    pub https_proxy_username: Option<String>,
    pub https_proxy_password: Option<String>,

    pub no_proxy_string: Option<String>,
    pub no_proxy_list: Vec<ByPassInfo>,
}

/// Regex to validate an IPv4 address.
static VALID_IP_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(([0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])\.){3}([0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])$",
    )
    .expect("Invalid IP regex")
});

impl RunnerWebProxy {
    /// Create a new `RunnerWebProxy` by reading proxy environment variables.
    pub fn new() -> Self {
        let http_proxy_raw = Self::read_env_ci("http_proxy", "HTTP_PROXY");
        let https_proxy_raw = Self::read_env_ci("https_proxy", "HTTPS_PROXY");
        let no_proxy_raw = Self::read_env_ci("no_proxy", "NO_PROXY");

        let mut proxy = RunnerWebProxy {
            http_proxy_address: None,
            http_proxy_username: None,
            http_proxy_password: None,
            https_proxy_address: None,
            https_proxy_username: None,
            https_proxy_password: None,
            no_proxy_string: None,
            no_proxy_list: Vec::new(),
        };

        if http_proxy_raw.is_none() && https_proxy_raw.is_none() {
            return proxy;
        }

        // Parse HTTP proxy
        if let Some(raw) = http_proxy_raw {
            let raw = raw.trim().to_string();
            if !raw.is_empty() {
                let address = Self::prepend_http_if_missing(&raw);
                if let Ok(parsed) = Url::parse(&address) {
                    proxy.http_proxy_address = Some(parsed.to_string());

                    // Normalize env vars
                    std::env::set_var("HTTP_PROXY", parsed.as_str());
                    std::env::set_var("http_proxy", parsed.as_str());

                    // Extract user info
                    let (username, password) = Self::extract_user_info(&parsed);
                    proxy.http_proxy_username = username;
                    proxy.http_proxy_password = password;
                }
            }
        }

        // Parse HTTPS proxy
        if let Some(raw) = https_proxy_raw {
            let raw = raw.trim().to_string();
            if !raw.is_empty() {
                let address = Self::prepend_http_if_missing(&raw);
                if let Ok(parsed) = Url::parse(&address) {
                    proxy.https_proxy_address = Some(parsed.to_string());

                    std::env::set_var("HTTPS_PROXY", parsed.as_str());
                    std::env::set_var("https_proxy", parsed.as_str());

                    let (username, password) = Self::extract_user_info(&parsed);
                    proxy.https_proxy_username = username;
                    proxy.https_proxy_password = password;
                }
            }
        }

        // Parse no_proxy
        if let Some(no_proxy) = no_proxy_raw {
            if !no_proxy.is_empty() {
                proxy.no_proxy_string = Some(no_proxy.clone());

                std::env::set_var("NO_PROXY", &no_proxy);
                std::env::set_var("no_proxy", &no_proxy);

                let mut seen = std::collections::HashSet::new();
                for entry in no_proxy.split(',') {
                    let trimmed = entry.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let lower = trimmed.to_lowercase();
                    if !seen.insert(lower) {
                        continue;
                    }

                    let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
                    let host = parts[0].to_string();
                    let port = parts.get(1).map(|p| p.to_string());

                    // Skip plain IP addresses (we don't support IP-based no_proxy)
                    if VALID_IP_REGEX.is_match(&host) {
                        continue;
                    }

                    proxy.no_proxy_list.push(ByPassInfo { host, port });
                }
            }
        }

        proxy
    }

    /// Determine the proxy URL to use for the given destination.
    /// Returns `None` if the destination should bypass the proxy.
    pub fn get_proxy_url(&self, destination: &Url) -> Option<Url> {
        if self.is_bypass(destination) {
            return None;
        }

        let proxy_address = if destination.scheme() == "https" {
            self.https_proxy_address.as_deref()
        } else {
            self.http_proxy_address.as_deref()
        };

        proxy_address.and_then(|addr| Url::parse(addr).ok())
    }

    /// Check if the given destination URL should bypass the proxy.
    pub fn is_bypass(&self, destination: &Url) -> bool {
        // If we have no proxy configured for this scheme, bypass
        match destination.scheme() {
            "https" if self.https_proxy_address.is_none() => return true,
            "http" if self.http_proxy_address.is_none() => return true,
            _ => {}
        }

        // Loopback is always bypassed
        if let Some(host) = destination.host_str() {
            let host_lower = host.to_lowercase();
            if host_lower == "localhost"
                || host_lower == "127.0.0.1"
                || host_lower == "::1"
                || host_lower == "[::1]"
            {
                return true;
            }
        }

        self.is_uri_in_bypass_list(destination)
    }

    /// Check if the destination matches any entry in the no_proxy list.
    fn is_uri_in_bypass_list(&self, destination: &Url) -> bool {
        let dest_host = match destination.host_str() {
            Some(h) => h.to_lowercase(),
            None => return false,
        };
        let dest_port = destination.port().map(|p| p.to_string());

        for bypass in &self.no_proxy_list {
            // Wildcard matches everything
            if bypass.host == "*" {
                return true;
            }

            // Check port match
            let port_match = match (&bypass.port, &dest_port) {
                (None, _) => true, // no port in bypass means match any port
                (Some(bp), Some(dp)) => bp == dp,
                (Some(_), None) => false,
            };

            // Check host match
            let bypass_host = bypass.host.to_lowercase();
            let host_match = if bypass_host.starts_with('.') {
                dest_host.ends_with(&bypass_host)
            } else {
                dest_host == bypass_host
                    || dest_host.ends_with(&format!(".{bypass_host}"))
            };

            if host_match && port_match {
                return true;
            }
        }

        false
    }

    /// Read an environment variable, trying the lowercase name first, then uppercase.
    fn read_env_ci(lower: &str, upper: &str) -> Option<String> {
        std::env::var(lower)
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var(upper).ok().filter(|s| !s.is_empty()))
    }

    /// Extract username and password from a proxy URL's user-info.
    fn extract_user_info(url: &Url) -> (Option<String>, Option<String>) {
        let username = if url.username().is_empty() {
            None
        } else {
            Some(
                percent_encoding::percent_decode_str(url.username())
                    .decode_utf8_lossy()
                    .to_string(),
            )
        };

        let password = url.password().map(|p| {
            percent_encoding::percent_decode_str(p)
                .decode_utf8_lossy()
                .to_string()
        });

        (username, password)
    }

    /// Prepend `http://` to a proxy address if it doesn't already have a scheme.
    fn prepend_http_if_missing(address: &str) -> String {
        if !address.starts_with("http://") && !address.starts_with("https://") {
            let prepended = format!("http://{address}");
            if Url::parse(&prepended).is_ok() {
                return prepended;
            }
        }
        address.to_string()
    }
}

impl Default for RunnerWebProxy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests manipulate env vars so they may interfere with each other
    // if run in parallel. Cargo runs tests in the same process by default.

    fn clear_proxy_env() {
        for var in &[
            "http_proxy",
            "HTTP_PROXY",
            "https_proxy",
            "HTTPS_PROXY",
            "no_proxy",
            "NO_PROXY",
        ] {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn no_proxy_env_set() {
        clear_proxy_env();
        let proxy = RunnerWebProxy::new();
        assert!(proxy.http_proxy_address.is_none());
        assert!(proxy.https_proxy_address.is_none());
    }

    #[test]
    fn prepend_http_if_missing() {
        assert_eq!(
            RunnerWebProxy::prepend_http_if_missing("127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            RunnerWebProxy::prepend_http_if_missing("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            RunnerWebProxy::prepend_http_if_missing("https://proxy.example.com"),
            "https://proxy.example.com"
        );
    }

    #[test]
    fn bypass_loopback() {
        clear_proxy_env();
        std::env::set_var("http_proxy", "http://proxy:8080");
        let proxy = RunnerWebProxy::new();
        let local = Url::parse("http://localhost/test").unwrap();
        assert!(proxy.is_bypass(&local));
        let local_ip = Url::parse("http://127.0.0.1/test").unwrap();
        assert!(proxy.is_bypass(&local_ip));
        clear_proxy_env();
    }

    #[test]
    fn bypass_wildcard() {
        clear_proxy_env();
        std::env::set_var("http_proxy", "http://proxy:8080");
        std::env::set_var("no_proxy", "*");
        let proxy = RunnerWebProxy::new();
        let url = Url::parse("http://anything.example.com/test").unwrap();
        assert!(proxy.is_bypass(&url));
        clear_proxy_env();
    }

    #[test]
    fn bypass_domain_suffix() {
        clear_proxy_env();
        std::env::set_var("http_proxy", "http://proxy:8080");
        std::env::set_var("no_proxy", ".example.com");
        let proxy = RunnerWebProxy::new();
        let url = Url::parse("http://foo.example.com/test").unwrap();
        assert!(proxy.is_bypass(&url));
        let url2 = Url::parse("http://other.com/test").unwrap();
        assert!(!proxy.is_bypass(&url2));
        clear_proxy_env();
    }

    #[test]
    fn extract_user_info_from_proxy_url() {
        let url = Url::parse("http://user:p%40ss@proxy:8080").unwrap();
        let (user, pass) = RunnerWebProxy::extract_user_info(&url);
        assert_eq!(user, Some("user".to_string()));
        assert_eq!(pass, Some("p@ss".to_string()));
    }
}
