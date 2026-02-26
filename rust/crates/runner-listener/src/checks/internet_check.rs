// Internet connectivity check.
// Maps to the C# Runner.Listener/Checks/InternetCheck.cs.
//
// Verifies basic internet connectivity by performing DNS resolution
// and HTTPS connectivity checks.

use super::check_extension::CheckResult;

const CHECK_NAME: &str = "Internet Connection";
const CHECK_DESCRIPTION: &str = "Check basic internet connectivity";
const DOC_URL: &str = "https://github.com/actions/runner/blob/main/docs/checks/internet.md";

/// Well-known hosts to check for basic connectivity.
const CHECK_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
];

pub struct InternetCheck;

impl InternetCheck {
    /// Run the internet connectivity check.
    pub async fn run_check() -> CheckResult {
        match Self::check_connectivity().await {
            Ok(_) => CheckResult::pass(CHECK_NAME, CHECK_DESCRIPTION)
                .with_doc_url(DOC_URL),
            Err(e) => CheckResult::fail(CHECK_NAME, CHECK_DESCRIPTION, e.to_string())
                .with_doc_url(DOC_URL),
        }
    }

    async fn check_connectivity() -> Result<(), anyhow::Error> {
        // Step 1: DNS resolution
        Self::check_dns().await?;

        // Step 2: HTTPS connectivity
        Self::check_https().await?;

        Ok(())
    }

    async fn check_dns() -> Result<(), anyhow::Error> {
        for host in CHECK_HOSTS {
            match tokio::net::lookup_host(format!("{}:443", host)).await {
                Ok(mut addrs) => {
                    if addrs.next().is_none() {
                        return Err(anyhow::anyhow!(
                            "DNS resolution for {} returned no addresses",
                            host
                        ));
                    }
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "DNS resolution failed for {}: {}. \
                         Please check your network configuration and DNS settings.",
                        host,
                        e
                    ));
                }
            }
        }
        Ok(())
    }

    async fn check_https() -> Result<(), anyhow::Error> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        let url = "https://github.com";
        let response = client.get(url).send().await.map_err(|e| {
            anyhow::anyhow!(
                "HTTPS connection to {} failed: {}. \
                 Please check your firewall and proxy settings.",
                url,
                e
            )
        })?;

        let status = response.status();
        if status.is_server_error() {
            return Err(anyhow::anyhow!(
                "HTTPS connection to {} returned server error: {}",
                url,
                status
            ));
        }

        Ok(())
    }
}

/// Check proxy configuration.
pub async fn check_proxy() -> CheckResult {
    let proxy_vars = ["https_proxy", "http_proxy", "HTTPS_PROXY", "HTTP_PROXY"];

    let mut proxy_found = false;
    let mut proxy_details = Vec::new();

    for var in &proxy_vars {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                proxy_found = true;
                proxy_details.push(format!("{}={}", var, val));
            }
        }
    }

    // Check no_proxy / NO_PROXY too
    for var in &["no_proxy", "NO_PROXY"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                proxy_details.push(format!("{}={}", var, val));
            }
        }
    }

    if proxy_found {
        let mut result = CheckResult::pass(
            "Proxy Configuration",
            "Check if proxy is configured",
        );
        result.detail = Some(proxy_details.join(", "));
        result
    } else {
        CheckResult::pass(
            "Proxy Configuration",
            "Check if proxy is configured (no proxy configured)",
        )
    }
}
