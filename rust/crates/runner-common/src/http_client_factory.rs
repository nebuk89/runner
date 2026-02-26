// HttpClientFactory mapping `HttpClientHandlerFactory.cs`.
// Creates HTTP clients with proxy and TLS configuration.

use anyhow::Result;
use reqwest::Client;
use runner_sdk::RunnerWebProxy;

/// Creates properly configured HTTP clients for the runner.
///
/// Maps `HttpClientHandlerFactory` in the C# runner.
pub struct HttpClientFactory;

impl HttpClientFactory {
    /// Create a new `reqwest::Client` configured with proxy and TLS settings.
    ///
    /// - If `GITHUB_ACTIONS_RUNNER_TLS_NO_VERIFY` is set, TLS certificate
    ///   verification is disabled (dangerous!).
    /// - HTTP and HTTPS proxy settings are read from the `RunnerWebProxy`.
    pub fn create_client(web_proxy: &RunnerWebProxy) -> Result<Client> {
        let mut builder = Client::builder();

        // Configure proxy
        if let Some(ref http_proxy) = web_proxy.http_proxy_address {
            let mut proxy = reqwest::Proxy::http(http_proxy)?;
            if let (Some(ref user), Some(ref pass)) =
                (&web_proxy.http_proxy_username, &web_proxy.http_proxy_password)
            {
                proxy = proxy.basic_auth(user, pass);
            }
            builder = builder.proxy(proxy);
        }

        if let Some(ref https_proxy) = web_proxy.https_proxy_address {
            let mut proxy = reqwest::Proxy::https(https_proxy)?;
            if let (Some(ref user), Some(ref pass)) = (
                &web_proxy.https_proxy_username,
                &web_proxy.https_proxy_password,
            ) {
                proxy = proxy.basic_auth(user, pass);
            }
            builder = builder.proxy(proxy);
        }

        // Configure no-proxy list
        if let Some(ref no_proxy_str) = web_proxy.no_proxy_string {
            if !no_proxy_str.is_empty() {
                builder = builder.no_proxy();
                // Re-add proxies with the no_proxy filter
                // reqwest handles NO_PROXY via the environment automatically,
                // so we ensure the env var is set
                std::env::set_var("NO_PROXY", no_proxy_str);
            }
        }

        // TLS verification
        if let Ok(val) = std::env::var("GITHUB_ACTIONS_RUNNER_TLS_NO_VERIFY") {
            if runner_sdk::StringUtil::convert_to_bool(&val) == Some(true) {
                builder = builder.danger_accept_invalid_certs(true);
            }
        }

        // Default user agent
        builder = builder.user_agent(format!(
            "GitHubActionsRunner-{}/{}",
            runner_sdk::build_constants::RunnerPackage::PACKAGE_NAME,
            runner_sdk::build_constants::RunnerPackage::VERSION,
        ));

        let client = builder.build()?;
        Ok(client)
    }

    /// Create a client with default proxy settings read from environment.
    pub fn create_default_client() -> Result<Client> {
        let proxy = RunnerWebProxy::new();
        Self::create_client(&proxy)
    }
}
