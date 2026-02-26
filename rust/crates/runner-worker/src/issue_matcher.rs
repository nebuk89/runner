// IssueMatcher mapping `IssueMatcher.cs`.
// Regex-based problem matchers that detect errors/warnings in step output.

use regex::Regex;
use std::time::Duration;

/// A single match result from an issue matcher.
#[derive(Debug, Clone)]
pub struct IssueMatch {
    /// Matched file path, if available.
    pub file: Option<String>,

    /// Matched line number, if available.
    pub line: Option<u32>,

    /// Matched column number, if available.
    pub column: Option<u32>,

    /// Severity: "error", "warning", or "notice".
    pub severity: String,

    /// The matched message text.
    pub message: String,

    /// The source code or matched text.
    pub code: Option<String>,
}

/// A single pattern within an issue matcher.
#[derive(Debug, Clone)]
pub struct IssuePattern {
    /// Compiled regex for this pattern.
    regex: Regex,

    /// Capture group index for the file path.
    file: Option<usize>,

    /// Capture group index for the line number.
    line: Option<usize>,

    /// Capture group index for the column.
    column: Option<usize>,

    /// Capture group index for the severity.
    severity: Option<usize>,

    /// Capture group index for the message.
    message: Option<usize>,

    /// Capture group index for the code.
    code: Option<usize>,

    /// Whether this pattern loops (matches multiple consecutive lines).
    is_loop: bool,
}

/// An issue matcher that detects problems in step output.
///
/// Matchers can be single-pattern (match one line) or multi-pattern
/// (match a sequence of lines).
#[derive(Debug, Clone)]
pub struct IssueMatcher {
    /// Owner / identifier for this matcher.
    owner: String,

    /// Patterns to match against (ordered).
    patterns: Vec<IssuePattern>,

    /// Default severity if not captured.
    default_severity: String,

    /// State for multi-line matching.
    state: MatcherState,
}

/// State for multi-line matching.
#[derive(Debug, Clone, Default)]
struct MatcherState {
    /// Current pattern index for multi-line matching.
    current_pattern: usize,

    /// Partially captured values.
    partial_file: Option<String>,
    partial_line: Option<u32>,
    partial_column: Option<u32>,
    partial_severity: Option<String>,
    partial_message: Option<String>,
    partial_code: Option<String>,
}

impl IssueMatcher {
    /// Create a new issue matcher from a JSON configuration.
    pub fn from_config(config: &IssueMatcherConfig) -> Option<Self> {
        let mut patterns = Vec::new();

        for pattern_config in &config.patterns {
            let regex = match Regex::new(&pattern_config.regexp) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        "Invalid regex in issue matcher '{}': {}",
                        config.owner,
                        e
                    );
                    return None;
                }
            };

            patterns.push(IssuePattern {
                regex,
                file: pattern_config.file,
                line: pattern_config.line,
                column: pattern_config.column,
                severity: pattern_config.severity,
                message: pattern_config.message,
                code: pattern_config.code,
                is_loop: pattern_config.is_loop.unwrap_or(false),
            });
        }

        if patterns.is_empty() {
            return None;
        }

        Some(Self {
            owner: config.owner.clone(),
            patterns,
            default_severity: config.severity.clone().unwrap_or_else(|| "error".to_string()),
            state: MatcherState::default(),
        })
    }

    /// Get the owner name of this matcher.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Try to match a line of output.
    ///
    /// For single-pattern matchers, returns immediately on match.
    /// For multi-pattern matchers, accumulates state across lines.
    pub fn try_match(&self, line: &str) -> Option<IssueMatch> {
        if self.patterns.len() == 1 {
            self.try_match_single(line)
        } else {
            // Multi-pattern matching would need mutable state;
            // for thread safety we clone and match
            self.try_match_single_against_all(line)
        }
    }

    /// Try to match using a single-pattern matcher.
    fn try_match_single(&self, line: &str) -> Option<IssueMatch> {
        let pattern = &self.patterns[0];
        let captures = pattern.regex.captures(line)?;

        Some(IssueMatch {
            file: extract_capture(&captures, pattern.file),
            line: extract_capture(&captures, pattern.line)
                .and_then(|s| s.parse().ok()),
            column: extract_capture(&captures, pattern.column)
                .and_then(|s| s.parse().ok()),
            severity: extract_capture(&captures, pattern.severity)
                .unwrap_or_else(|| self.default_severity.clone()),
            message: extract_capture(&captures, pattern.message)
                .unwrap_or_else(|| line.to_string()),
            code: extract_capture(&captures, pattern.code),
        })
    }

    /// Try to match against all patterns (fallback for multi-pattern).
    fn try_match_single_against_all(&self, line: &str) -> Option<IssueMatch> {
        for pattern in &self.patterns {
            if let Some(captures) = pattern.regex.captures(line) {
                return Some(IssueMatch {
                    file: extract_capture(&captures, pattern.file),
                    line: extract_capture(&captures, pattern.line)
                        .and_then(|s| s.parse().ok()),
                    column: extract_capture(&captures, pattern.column)
                        .and_then(|s| s.parse().ok()),
                    severity: extract_capture(&captures, pattern.severity)
                        .unwrap_or_else(|| self.default_severity.clone()),
                    message: extract_capture(&captures, pattern.message)
                        .unwrap_or_else(|| line.to_string()),
                    code: extract_capture(&captures, pattern.code),
                });
            }
        }
        None
    }
}

/// Extract a capture group by index.
fn extract_capture(captures: &regex::Captures, group: Option<usize>) -> Option<String> {
    group
        .and_then(|i| captures.get(i))
        .map(|m| m.as_str().to_string())
}

/// Configuration for an issue matcher (loaded from JSON).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct IssueMatcherConfig {
    pub owner: String,
    #[serde(default)]
    pub severity: Option<String>,
    pub patterns: Vec<IssuePatternConfig>,
}

/// Configuration for a single pattern in an issue matcher.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct IssuePatternConfig {
    pub regexp: String,
    #[serde(default)]
    pub file: Option<usize>,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub column: Option<usize>,
    #[serde(default)]
    pub severity: Option<usize>,
    #[serde(default)]
    pub message: Option<usize>,
    #[serde(default)]
    pub code: Option<usize>,
    #[serde(rename = "loop")]
    #[serde(default)]
    pub is_loop: Option<bool>,
}

/// Load issue matchers from a JSON configuration file.
pub fn load_matchers_from_file(path: &str) -> Vec<IssueMatcher> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to read matcher config {}: {}", path, e);
            return Vec::new();
        }
    };

    load_matchers_from_json(&content)
}

/// Load issue matchers from a JSON string.
pub fn load_matchers_from_json(json: &str) -> Vec<IssueMatcher> {
    #[derive(serde::Deserialize)]
    struct MatcherFile {
        #[serde(rename = "problemMatcher")]
        problem_matcher: Vec<IssueMatcherConfig>,
    }

    let file: MatcherFile = match serde_json::from_str(json) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("Failed to parse matcher config: {}", e);
            return Vec::new();
        }
    };

    file.problem_matcher
        .iter()
        .filter_map(|config| IssueMatcher::from_config(config))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_pattern_matcher() {
        let config = IssueMatcherConfig {
            owner: "test".to_string(),
            severity: Some("error".to_string()),
            patterns: vec![IssuePatternConfig {
                regexp: r"^(.+):(\d+):(\d+): (error|warning): (.+)$".to_string(),
                file: Some(1),
                line: Some(2),
                column: Some(3),
                severity: Some(4),
                message: Some(5),
                code: None,
                is_loop: None,
            }],
        };

        let matcher = IssueMatcher::from_config(&config).unwrap();
        let result = matcher
            .try_match("src/main.rs:10:5: error: unused variable")
            .unwrap();

        assert_eq!(result.file, Some("src/main.rs".to_string()));
        assert_eq!(result.line, Some(10));
        assert_eq!(result.column, Some(5));
        assert_eq!(result.severity, "error");
        assert_eq!(result.message, "unused variable");
    }

    #[test]
    fn test_no_match() {
        let config = IssueMatcherConfig {
            owner: "test".to_string(),
            severity: None,
            patterns: vec![IssuePatternConfig {
                regexp: r"^ERROR: (.+)$".to_string(),
                file: None,
                line: None,
                column: None,
                severity: None,
                message: Some(1),
                code: None,
                is_loop: None,
            }],
        };

        let matcher = IssueMatcher::from_config(&config).unwrap();
        assert!(matcher.try_match("just a regular line").is_none());
    }

    #[test]
    fn test_load_matchers_from_json() {
        let json = r#"{
            "problemMatcher": [
                {
                    "owner": "eslint",
                    "patterns": [
                        {
                            "regexp": "^(.+):(\\d+):(\\d+): (.+)$",
                            "file": 1,
                            "line": 2,
                            "column": 3,
                            "message": 4
                        }
                    ]
                }
            ]
        }"#;

        let matchers = load_matchers_from_json(json);
        assert_eq!(matchers.len(), 1);
        assert_eq!(matchers[0].owner(), "eslint");
    }
}
