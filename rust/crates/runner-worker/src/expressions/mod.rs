// expressions/mod.rs mapping expression evaluation in `ExpressionManager.cs`.
// Evaluates GitHub Actions workflow expressions: always(), success(), failure(),
// cancelled(), hashFiles(), and general ${{ ... }} interpolation.

use std::collections::HashMap;

use runner_common::util::task_result_util::TaskResult;

/// Evaluate a step condition expression.
///
/// Supported status functions:
/// - `success()` — true if all previous steps succeeded
/// - `failure()` — true if any previous step failed
/// - `always()` — always true
/// - `cancelled()` — true if the job was cancelled
///
/// The condition string is the raw `if:` value from the workflow YAML.
/// Returns `true` if the step should execute, `false` if it should be skipped.
pub fn evaluate_condition(
    condition: &str,
    job_status: TaskResult,
    is_cancelled: bool,
    expression_context: &serde_json::Value,
) -> bool {
    let trimmed = condition.trim();

    // Empty condition defaults to success()
    if trimmed.is_empty() {
        return matches!(job_status, TaskResult::Succeeded);
    }

    // Normalize: strip outer ${{ }} if present
    let expr = if trimmed.starts_with("${{") && trimmed.ends_with("}}") {
        trimmed[3..trimmed.len() - 2].trim()
    } else {
        trimmed
    };

    // Check for status functions (case-insensitive)
    let lower = expr.to_lowercase();

    // Handle simple status function calls
    if lower == "always()" {
        return true;
    }

    if lower == "cancelled()" {
        return is_cancelled;
    }

    if lower == "failure()" {
        return matches!(job_status, TaskResult::Failed);
    }

    if lower == "success()" {
        return matches!(job_status, TaskResult::Succeeded);
    }

    // Handle compound expressions with status functions
    if contains_status_function(&lower) {
        return evaluate_compound_condition(expr, job_status, is_cancelled, expression_context);
    }

    // If no status function is referenced, implicitly wrap with success() &&
    // i.e., the step only runs if previous steps succeeded AND the expression is true
    if !matches!(job_status, TaskResult::Succeeded) {
        return false;
    }

    evaluate_expression(expr, expression_context)
}

/// Check if a condition string contains a status function.
fn contains_status_function(lower: &str) -> bool {
    lower.contains("always()")
        || lower.contains("cancelled()")
        || lower.contains("failure()")
        || lower.contains("success()")
}

/// Evaluate a compound condition that mixes status functions with other expressions.
fn evaluate_compound_condition(
    expr: &str,
    job_status: TaskResult,
    is_cancelled: bool,
    expression_context: &serde_json::Value,
) -> bool {
    let lower = expr.to_lowercase();

    // Handle common patterns
    // "always() && ..."
    if lower.starts_with("always()") {
        if let Some(rest) = lower.strip_prefix("always()") {
            let rest = rest.trim();
            if rest.is_empty() {
                return true;
            }
            if let Some(rest) = rest.strip_prefix("&&") {
                return evaluate_expression(rest.trim(), expression_context);
            }
        }
        return true;
    }

    // "failure() && ..."
    if lower.starts_with("failure()") {
        if !matches!(job_status, TaskResult::Failed) {
            return false;
        }
        if let Some(rest) = lower.strip_prefix("failure()") {
            let rest = rest.trim();
            if rest.is_empty() {
                return true;
            }
            if let Some(rest) = rest.strip_prefix("&&") {
                return evaluate_expression(rest.trim(), expression_context);
            }
        }
        return true;
    }

    // "cancelled() && ..."
    if lower.starts_with("cancelled()") {
        if !is_cancelled {
            return false;
        }
        if let Some(rest) = lower.strip_prefix("cancelled()") {
            let rest = rest.trim();
            if rest.is_empty() {
                return true;
            }
            if let Some(rest) = rest.strip_prefix("&&") {
                return evaluate_expression(rest.trim(), expression_context);
            }
        }
        return true;
    }

    // "success() && ..."
    if lower.starts_with("success()") {
        if !matches!(job_status, TaskResult::Succeeded) {
            return false;
        }
        if let Some(rest) = lower.strip_prefix("success()") {
            let rest = rest.trim();
            if rest.is_empty() {
                return true;
            }
            if let Some(rest) = rest.strip_prefix("&&") {
                return evaluate_expression(rest.trim(), expression_context);
            }
        }
        return true;
    }

    // Handle "!cancelled()" pattern
    if lower.contains("!cancelled()") || lower.contains("! cancelled()") {
        if is_cancelled {
            return false;
        }
        // Remove the !cancelled() and evaluate the rest
        let cleaned = lower
            .replace("!cancelled()", "true")
            .replace("! cancelled()", "true");
        return evaluate_expression(&cleaned, expression_context);
    }

    // Handle || (OR) patterns
    if lower.contains("||") {
        let parts: Vec<&str> = expr.split("||").collect();
        for part in parts {
            let part = part.trim();
            let part_lower = part.to_lowercase();
            let result = if part_lower == "always()" {
                true
            } else if part_lower == "failure()" {
                matches!(job_status, TaskResult::Failed)
            } else if part_lower == "cancelled()" {
                is_cancelled
            } else if part_lower == "success()" {
                matches!(job_status, TaskResult::Succeeded)
            } else {
                evaluate_expression(part, expression_context)
            };

            if result {
                return true;
            }
        }
        return false;
    }

    // Fallback: evaluate as a simple expression
    evaluate_expression(expr, expression_context)
}

/// Evaluate a simple expression against the expression context.
///
/// Supports:
/// - Boolean literals: true, false
/// - String comparison: `github.event_name == 'push'`
/// - Context access: `env.MY_VAR`
/// - Negation: `!expr`
/// - contains(): `contains(github.event.head_commit.message, '[skip ci]')`
/// - startsWith(), endsWith()
fn evaluate_expression(expr: &str, context: &serde_json::Value) -> bool {
    let trimmed = expr.trim();

    if trimmed.is_empty() || trimmed == "true" {
        return true;
    }

    if trimmed == "false" {
        return false;
    }

    // Handle negation
    if let Some(inner) = trimmed.strip_prefix('!') {
        return !evaluate_expression(inner.trim(), context);
    }

    // Handle == comparison
    if let Some((left, right)) = split_comparison(trimmed, "==") {
        let left_val = resolve_value(left.trim(), context);
        let right_val = resolve_value(right.trim(), context);
        return left_val.eq_ignore_ascii_case(&right_val);
    }

    // Handle != comparison
    if let Some((left, right)) = split_comparison(trimmed, "!=") {
        let left_val = resolve_value(left.trim(), context);
        let right_val = resolve_value(right.trim(), context);
        return !left_val.eq_ignore_ascii_case(&right_val);
    }

    // Handle && (AND)
    if trimmed.contains("&&") {
        let parts: Vec<&str> = trimmed.split("&&").collect();
        return parts
            .iter()
            .all(|p| evaluate_expression(p.trim(), context));
    }

    // Handle || (OR)
    if trimmed.contains("||") {
        let parts: Vec<&str> = trimmed.split("||").collect();
        return parts
            .iter()
            .any(|p| evaluate_expression(p.trim(), context));
    }

    // Handle contains(haystack, needle)
    if let Some(args) = extract_function_args(trimmed, "contains") {
        if let Some((haystack, needle)) = split_function_args(&args) {
            let h = resolve_value(haystack.trim(), context).to_lowercase();
            let n = resolve_value(needle.trim(), context).to_lowercase();
            return h.contains(&n);
        }
    }

    // Handle startsWith(string, prefix)
    if let Some(args) = extract_function_args(trimmed, "startswith") {
        if let Some((s, prefix)) = split_function_args(&args) {
            let sv = resolve_value(s.trim(), context).to_lowercase();
            let pv = resolve_value(prefix.trim(), context).to_lowercase();
            return sv.starts_with(&pv);
        }
    }

    // Handle endsWith(string, suffix)
    if let Some(args) = extract_function_args(trimmed, "endswith") {
        if let Some((s, suffix)) = split_function_args(&args) {
            let sv = resolve_value(s.trim(), context).to_lowercase();
            let fv = resolve_value(suffix.trim(), context).to_lowercase();
            return sv.ends_with(&fv);
        }
    }

    // Handle hashFiles() — always true for condition evaluation purposes
    let lower = trimmed.to_lowercase();
    if lower.starts_with("hashfiles(") {
        return true;
    }

    // Try to resolve as a context value and check truthiness
    let resolved = resolve_value(trimmed, context);
    is_truthy(&resolved)
}

/// Resolve a value from the expression context.
///
/// Handles:
/// - String literals: 'value'
/// - Numeric literals: 42
/// - Context paths: github.event_name, env.MY_VAR
fn resolve_value(expr: &str, context: &serde_json::Value) -> String {
    let trimmed = expr.trim();

    // String literal
    if (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('"') && trimmed.ends_with('"'))
    {
        return trimmed[1..trimmed.len() - 1].to_string();
    }

    // Numeric literal
    if trimmed.parse::<f64>().is_ok() {
        return trimmed.to_string();
    }

    // Boolean literals
    if trimmed == "true" {
        return "true".to_string();
    }
    if trimmed == "false" {
        return "false".to_string();
    }

    // Context path: navigate the JSON value
    let parts: Vec<&str> = trimmed.split('.').collect();
    let mut current = context;

    for part in &parts {
        // Handle bracket notation: steps['step-id']
        if let Some(bracket_start) = part.find('[') {
            let key = &part[..bracket_start];
            if !key.is_empty() {
                current = match current.get(key) {
                    Some(v) => v,
                    None => return String::new(),
                };
            }
            let inner = &part[bracket_start + 1..part.len() - 1];
            let inner = inner.trim_matches('\'').trim_matches('"');
            current = match current.get(inner) {
                Some(v) => v,
                None => return String::new(),
            };
        } else {
            current = match current.get(*part) {
                Some(v) => v,
                None => return String::new(),
            };
        }
    }

    match current {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Check if a string value is "truthy" in GitHub Actions expressions.
fn is_truthy(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value == "0" || value == "false" || value == "null" {
        return false;
    }
    true
}

/// Split a comparison expression on an operator.
fn split_comparison<'a>(expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    // Find the operator outside of string literals
    let mut in_string = false;
    let mut string_char = ' ';
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();

    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            if c == string_char {
                in_string = false;
            }
        } else if c == '\'' || c == '"' {
            in_string = true;
            string_char = c;
        } else if i + op_bytes.len() <= bytes.len() && &bytes[i..i + op_bytes.len()] == op_bytes {
            return Some((&expr[..i], &expr[i + op.len()..]));
        }
        i += 1;
    }

    None
}

/// Extract function arguments from a function call like "funcName(args)".
fn extract_function_args(expr: &str, func_name: &str) -> Option<String> {
    let lower = expr.to_lowercase();
    let prefix = format!("{}(", func_name);
    if lower.starts_with(&prefix) && expr.ends_with(')') {
        let start = prefix.len();
        let end = expr.len() - 1;
        Some(expr[start..end].to_string())
    } else {
        None
    }
}

/// Split function arguments (handles one comma separator).
fn split_function_args(args: &str) -> Option<(String, String)> {
    // Find comma outside of string literals
    let mut depth = 0;
    let mut in_string = false;
    let mut string_char = ' ';

    for (i, c) in args.char_indices() {
        if in_string {
            if c == string_char {
                in_string = false;
            }
        } else if c == '\'' || c == '"' {
            in_string = true;
            string_char = c;
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        } else if c == ',' && depth == 0 {
            return Some((args[..i].to_string(), args[i + 1..].to_string()));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_condition() {
        let ctx = serde_json::json!({});
        assert!(evaluate_condition("success()", TaskResult::Succeeded, false, &ctx));
        assert!(!evaluate_condition("success()", TaskResult::Failed, false, &ctx));
    }

    #[test]
    fn test_failure_condition() {
        let ctx = serde_json::json!({});
        assert!(evaluate_condition("failure()", TaskResult::Failed, false, &ctx));
        assert!(!evaluate_condition("failure()", TaskResult::Succeeded, false, &ctx));
    }

    #[test]
    fn test_always_condition() {
        let ctx = serde_json::json!({});
        assert!(evaluate_condition("always()", TaskResult::Succeeded, false, &ctx));
        assert!(evaluate_condition("always()", TaskResult::Failed, false, &ctx));
        assert!(evaluate_condition("always()", TaskResult::Canceled, true, &ctx));
    }

    #[test]
    fn test_cancelled_condition() {
        let ctx = serde_json::json!({});
        assert!(evaluate_condition("cancelled()", TaskResult::Canceled, true, &ctx));
        assert!(!evaluate_condition("cancelled()", TaskResult::Succeeded, false, &ctx));
    }

    #[test]
    fn test_empty_defaults_to_success() {
        let ctx = serde_json::json!({});
        assert!(evaluate_condition("", TaskResult::Succeeded, false, &ctx));
        assert!(!evaluate_condition("", TaskResult::Failed, false, &ctx));
    }

    #[test]
    fn test_wrapped_condition() {
        let ctx = serde_json::json!({});
        assert!(evaluate_condition("${{ always() }}", TaskResult::Failed, false, &ctx));
    }

    #[test]
    fn test_equality_expression() {
        let ctx = serde_json::json!({
            "github": {
                "event_name": "push"
            }
        });
        assert!(evaluate_condition(
            "github.event_name == 'push'",
            TaskResult::Succeeded,
            false,
            &ctx
        ));
        assert!(!evaluate_condition(
            "github.event_name == 'pull_request'",
            TaskResult::Succeeded,
            false,
            &ctx
        ));
    }

    #[test]
    fn test_contains_function() {
        let ctx = serde_json::json!({
            "github": {
                "ref": "refs/heads/main"
            }
        });
        assert!(evaluate_condition(
            "contains(github.ref, 'main')",
            TaskResult::Succeeded,
            false,
            &ctx
        ));
    }

    #[test]
    fn test_starts_with_function() {
        let ctx = serde_json::json!({
            "github": {
                "ref": "refs/heads/main"
            }
        });
        assert!(evaluate_condition(
            "startsWith(github.ref, 'refs/heads/')",
            TaskResult::Succeeded,
            false,
            &ctx
        ));
    }

    #[test]
    fn test_negation() {
        let ctx = serde_json::json!({
            "github": {
                "event_name": "push"
            }
        });
        assert!(evaluate_condition(
            "github.event_name != 'pull_request'",
            TaskResult::Succeeded,
            false,
            &ctx
        ));
    }

    #[test]
    fn test_implicit_success_gate() {
        let ctx = serde_json::json!({
            "env": { "RUN_TESTS": "true" }
        });
        // With failure status, even true expressions should not run
        assert!(!evaluate_condition(
            "env.RUN_TESTS == 'true'",
            TaskResult::Failed,
            false,
            &ctx
        ));
    }

    #[test]
    fn test_resolve_string_literal() {
        let ctx = serde_json::json!({});
        assert_eq!(resolve_value("'hello'", &ctx), "hello");
    }

    #[test]
    fn test_resolve_context_path() {
        let ctx = serde_json::json!({
            "github": {
                "repository": "owner/repo"
            }
        });
        assert_eq!(resolve_value("github.repository", &ctx), "owner/repo");
    }

    #[test]
    fn test_is_truthy() {
        assert!(is_truthy("hello"));
        assert!(is_truthy("1"));
        assert!(is_truthy("true"));
        assert!(!is_truthy(""));
        assert!(!is_truthy("0"));
        assert!(!is_truthy("false"));
    }
}
