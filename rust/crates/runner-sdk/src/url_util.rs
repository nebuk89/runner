use crate::string_util::StringUtil;
use reqwest::header::HeaderMap;
use url::Url;

/// URL utility functions mapping `UrlUtil.cs`.
pub struct UrlUtil;

impl UrlUtil {
    /// Checks whether the given URL points to a hosted (github.com / ghe.com) server.
    ///
    /// Returns `false` if the `GITHUB_ACTIONS_RUNNER_FORCE_GHES` env var is truthy,
    /// which forces the runner to treat the URL as a GitHub Enterprise Server instance.
    pub fn is_hosted_server(url: &Url) -> bool {
        // If the force-GHES flag is set, treat everything as non-hosted
        if let Ok(val) = std::env::var("GITHUB_ACTIONS_RUNNER_FORCE_GHES") {
            if StringUtil::convert_to_bool(&val) == Some(true) {
                return false;
            }
        }

        let host = match url.host_str() {
            Some(h) => h.to_lowercase(),
            None => return false,
        };

        host == "github.com"
            || host == "www.github.com"
            || host == "github.localhost"
            || host.ends_with(".ghe.localhost")
            || host.ends_with(".ghe.com")
    }

    /// Embed username and password into a URL for credential-based access.
    ///
    /// If both `username` and `password` are empty, returns the URL unchanged.
    /// If `username` is empty but `password` is provided, uses `"emptyusername"`.
    pub fn get_credential_embedded_url(url: &Url, username: &str, password: &str) -> Url {
        if username.is_empty() && password.is_empty() {
            return url.clone();
        }

        let mut result = url.clone();

        let effective_username = if username.is_empty() {
            "emptyusername"
        } else {
            username
        };

        result
            .set_username(effective_username)
            .expect("Failed to set username on URL");

        if !password.is_empty() {
            result
                .set_password(Some(password))
                .expect("Failed to set password on URL");
        }

        result
    }

    /// Extract the `x-github-request-id` header value from an HTTP response's headers.
    pub fn get_github_request_id(headers: &HeaderMap) -> Option<String> {
        headers
            .get("x-github-request-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }

    /// Extract the `x-vss-e2eid` header value from an HTTP response's headers.
    pub fn get_vss_request_id(headers: &HeaderMap) -> Option<String> {
        headers
            .get("x-vss-e2eid")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_hosted_github_com() {
        let url = Url::parse("https://github.com/owner/repo").unwrap();
        // Temporarily unset the force flag
        std::env::remove_var("GITHUB_ACTIONS_RUNNER_FORCE_GHES");
        assert!(UrlUtil::is_hosted_server(&url));
    }

    #[test]
    fn is_hosted_ghe_com() {
        let url = Url::parse("https://mycompany.ghe.com/owner/repo").unwrap();
        std::env::remove_var("GITHUB_ACTIONS_RUNNER_FORCE_GHES");
        assert!(UrlUtil::is_hosted_server(&url));
    }

    #[test]
    fn is_not_hosted_custom() {
        let url = Url::parse("https://github.mycompany.com/owner/repo").unwrap();
        std::env::remove_var("GITHUB_ACTIONS_RUNNER_FORCE_GHES");
        assert!(!UrlUtil::is_hosted_server(&url));
    }

    #[test]
    fn credential_embedded_url_both() {
        let url = Url::parse("https://github.com/repo").unwrap();
        let result = UrlUtil::get_credential_embedded_url(&url, "user", "pass");
        assert_eq!(result.username(), "user");
        assert_eq!(result.password(), Some("pass"));
    }

    #[test]
    fn credential_embedded_url_empty_username() {
        let url = Url::parse("https://github.com/repo").unwrap();
        let result = UrlUtil::get_credential_embedded_url(&url, "", "pass");
        assert_eq!(result.username(), "emptyusername");
        assert_eq!(result.password(), Some("pass"));
    }

    #[test]
    fn credential_embedded_url_both_empty() {
        let url = Url::parse("https://github.com/repo").unwrap();
        let result = UrlUtil::get_credential_embedded_url(&url, "", "");
        assert_eq!(result.as_str(), "https://github.com/repo");
    }

    #[test]
    fn get_github_request_id_present() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-request-id", "abc-123".parse().unwrap());
        assert_eq!(
            UrlUtil::get_github_request_id(&headers),
            Some("abc-123".to_string())
        );
    }

    #[test]
    fn get_github_request_id_missing() {
        let headers = HeaderMap::new();
        assert_eq!(UrlUtil::get_github_request_id(&headers), None);
    }
}
