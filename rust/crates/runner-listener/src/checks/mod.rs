// Checks module — system diagnostic checks for connectivity, git, node, etc.
// Maps to the C# Runner.Listener/Checks/ directory.

pub mod check_extension;
pub mod actions_check;
pub mod git_check;
pub mod internet_check;
pub mod nodejs_check;

use check_extension::CheckResult;
use runner_sdk::TraceWriter;

/// Run all diagnostic checks and return the results.
/// If `url` is provided, it will be used to check connectivity to the Actions service.
/// Otherwise, the server URL from the runner configuration is used.
pub async fn run_all_checks(
    url: Option<&str>,
    trace: &runner_common::Tracing,
) -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Internet connectivity
    trace.info("Running internet check...");
    let internet_result = internet_check::InternetCheck::run_check().await;
    results.push(internet_result);

    // Actions service connectivity
    let actions_url = url.unwrap_or("https://github.com");
    trace.info(&format!("Running Actions check against {}...", actions_url));
    let actions_result = actions_check::ActionsCheck::run_check(actions_url).await;
    results.push(actions_result);

    // Git check
    trace.info("Running Git check...");
    let git_result = git_check::GitCheck::run_check().await;
    results.push(git_result);

    // Node.js check
    trace.info("Running Node.js check...");
    let node_result = nodejs_check::NodeJsCheck::run_check().await;
    results.push(node_result);

    results
}

/// Format check results for display.
pub fn format_check_results(results: &[CheckResult]) -> String {
    let mut output = String::new();
    output.push_str("\n----------------------------------------------\n");
    output.push_str("  Runner Diagnostic Checks\n");
    output.push_str("----------------------------------------------\n\n");

    let pass_count = results.iter().filter(|r| r.passed).count();
    let fail_count = results.iter().filter(|r| !r.passed).count();

    for result in results {
        let status = if result.passed { "Pass" } else { "Fail" };
        output.push_str(&format!("  [{}] {}\n", status, result.name));
        if !result.description.is_empty() {
            output.push_str(&format!("        {}\n", result.description));
        }
        if let Some(ref detail) = result.detail {
            output.push_str(&format!("        {}\n", detail));
        }
        output.push('\n');
    }

    output.push_str(&format!(
        "  {} passed, {} failed\n",
        pass_count, fail_count
    ));
    output.push_str("----------------------------------------------\n");

    output
}
