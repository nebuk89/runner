// Actions connectivity check.
// Maps to the C# Runner.Listener/Checks/ActionsCheck.cs.
//
// Verifies that the runner can reach the GitHub Actions service endpoints.

use super::check_extension::CheckResult;
use url::Url;

const CHECK_NAME: &str = "Actions Connection";
const CHECK_DESCRIPTION: &str = "Check connectivity to GitHub Actions service";
const DOC_URL: &str = "https://github.com/actions/runner/blob/main/docs/checks/actions.md";

pub struct ActionsCheck;

impl ActionsCheck {
    /// Run the Actions connectivity check against the given server URL.
    pub async fn run_check(server_url: &str) -> CheckResult {
        let result = Self::check_connectivity(server_url).await;

        match result {
            Ok(_detail) => CheckResult::pass(CHECK_NAME, CHECK_DESCRIPTION)
                .with_doc_url(DOC_URL),
            Err(e) => CheckResult::fail(
                CHECK_NAME,
                CHECK_DESCRIPTION,
                format!("Failed to connect to {}: {}", server_url, e),
            )
            .with_doc_url(DOC_URL),
        }
    }

    async fn check_connectivity(server_url: &str) -> Result<String, anyhow::Error> {
        let url = Url::parse(server_url)?;

        // Build the API URL
        let api_url = Self::get_api_url(&url)?;

        // Try to connect
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        let response = client.get(api_url.as_str()).send().await?;
        let status = response.status();

        if status.is_success() || status.as_u16() == 401 || status.as_u16() == 403 {
            // 401/403 means we reached the server (just not authenticated)
            Ok(format!(
                "Successfully connected to {} (status: {})",
                api_url, status
            ))
        } else {
            Err(anyhow::anyhow!(
                "Unexpected status code {} from {}",
                status,
                api_url
            ))
        }
    }

    fn get_api_url(url: &Url) -> Result<Url, anyhow::Error> {
        let host = url.host_str().unwrap_or("");

        // GitHub.com
        if host == "github.com" || host.ends_with(".github.com") {
            return Ok(Url::parse("https://api.github.com")?);
        }

        // GitHub Enterprise Server (GHES)
        // API endpoint is at /api/v3
        let mut api_url = url.clone();
        api_url.set_path("/api/v3");
        Ok(api_url)
    }
}

/// Check DNS resolution for GitHub Actions domains.
pub async fn check_actions_dns() -> CheckResult {
    let domains = [
        "github.com",
        "api.github.com",
        "codeload.github.com",
        "ghcr.io",
        "pipelines.actions.githubusercontent.com",
        "results-receiver.actions.githubusercontent.com",
    ];

    let mut failures = Vec::new();

    for domain in &domains {
        match tokio::net::lookup_host(format!("{}:443", domain)).await {
            Ok(_) => {}
            Err(e) => {
                failures.push(format!("{}: {}", domain, e));
            }
        }
    }

    if failures.is_empty() {
        CheckResult::pass(
            "Actions DNS",
            "DNS resolution for GitHub Actions domains",
        )
        .with_doc_url(DOC_URL)
    } else {
        CheckResult::fail(
            "Actions DNS",
            "DNS resolution for GitHub Actions domains",
            format!("Failed to resolve: {}", failures.join(", ")),
        )
        .with_doc_url(DOC_URL)
    }
}
