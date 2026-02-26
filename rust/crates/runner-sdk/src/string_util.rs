use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// String utility functions mapping `StringUtil.cs`.
pub struct StringUtil;

impl StringUtil {
    /// Serialize a value to a pretty-printed JSON string.
    pub fn convert_to_json<T: Serialize>(value: &T) -> String {
        serde_json::to_string_pretty(value).unwrap_or_else(|e| {
            panic!("Failed to serialize value to JSON: {e}");
        })
    }

    /// Deserialize a JSON string into a value of type `T`.
    pub fn convert_from_json<T: DeserializeOwned>(json: &str) -> Result<T> {
        let value = serde_json::from_str(json)?;
        Ok(value)
    }

    /// Convert a string to a boolean.
    ///
    /// Valid true values: `"1"`, `"true"`, `"$true"` (case-insensitive).
    /// Valid false values: `"0"`, `"false"`, `"$false"` (case-insensitive).
    /// Returns `None` for unrecognized values.
    pub fn convert_to_bool(value: &str) -> Option<bool> {
        if value.is_empty() {
            return None;
        }
        match value.to_lowercase().as_str() {
            "1" | "true" | "$true" => Some(true),
            "0" | "false" | "$false" => Some(false),
            _ => None,
        }
    }

    /// Replace characters that are invalid in file names with `_`.
    ///
    /// This replaces common invalid filename characters across platforms:
    /// `< > : " / \ | ? *` and ASCII control characters (0x00..0x1F).
    pub fn format_into_safe_filename(name: &str) -> String {
        let invalid_chars: &[char] = &[
            '<', '>', ':', '"', '/', '\\', '|', '?', '*',
        ];
        let mut result = String::with_capacity(name.len());
        for ch in name.chars() {
            if invalid_chars.contains(&ch) || (ch as u32) < 0x20 {
                result.push('_');
            } else {
                result.push(ch);
            }
        }
        result
    }

    /// Returns the portion of `input` before the first occurrence of `separator`.
    /// If `separator` is not found, returns the entire string.
    pub fn sub_string_before<'a>(input: &'a str, separator: char) -> &'a str {
        match input.find(separator) {
            Some(idx) => &input[..idx],
            None => input,
        }
    }

    /// Sanitize a user-agent header string by replacing parentheses with brackets.
    pub fn sanitize_user_agent_header(header: &str) -> String {
        header.replace('(', "[").replace(')', "]").trim().to_string()
    }

    /// Return a prefix substring of at most `count` characters.
    pub fn substring_prefix(value: &str, count: usize) -> &str {
        if count >= value.len() {
            value
        } else {
            // Find the char boundary at `count` bytes—but we want char count, not byte count
            let end = value
                .char_indices()
                .nth(count)
                .map(|(idx, _)| idx)
                .unwrap_or(value.len());
            &value[..end]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestObj {
        name: String,
        value: i32,
    }

    #[test]
    fn roundtrip_json() {
        let obj = TestObj {
            name: "test".to_string(),
            value: 42,
        };
        let json = StringUtil::convert_to_json(&obj);
        assert!(json.contains("\"name\": \"test\""));
        let parsed: TestObj = StringUtil::convert_from_json(&json).unwrap();
        assert_eq!(parsed, obj);
    }

    #[test]
    fn convert_to_bool_true_values() {
        assert_eq!(StringUtil::convert_to_bool("1"), Some(true));
        assert_eq!(StringUtil::convert_to_bool("true"), Some(true));
        assert_eq!(StringUtil::convert_to_bool("True"), Some(true));
        assert_eq!(StringUtil::convert_to_bool("$true"), Some(true));
        assert_eq!(StringUtil::convert_to_bool("$True"), Some(true));
    }

    #[test]
    fn convert_to_bool_false_values() {
        assert_eq!(StringUtil::convert_to_bool("0"), Some(false));
        assert_eq!(StringUtil::convert_to_bool("false"), Some(false));
        assert_eq!(StringUtil::convert_to_bool("$false"), Some(false));
    }

    #[test]
    fn convert_to_bool_unknown() {
        assert_eq!(StringUtil::convert_to_bool(""), None);
        assert_eq!(StringUtil::convert_to_bool("yes"), None);
        assert_eq!(StringUtil::convert_to_bool("no"), None);
    }

    #[test]
    fn safe_filename() {
        assert_eq!(
            StringUtil::format_into_safe_filename("hello<world>:test"),
            "hello_world__test"
        );
        assert_eq!(
            StringUtil::format_into_safe_filename("normal_name.txt"),
            "normal_name.txt"
        );
    }

    #[test]
    fn sub_string_before_found() {
        assert_eq!(StringUtil::sub_string_before("hello:world", ':'), "hello");
    }

    #[test]
    fn sub_string_before_not_found() {
        assert_eq!(
            StringUtil::sub_string_before("helloworld", ':'),
            "helloworld"
        );
    }

    #[test]
    fn sanitize_user_agent() {
        assert_eq!(
            StringUtil::sanitize_user_agent_header("(Linux 5.4)"),
            "[Linux 5.4]"
        );
    }
}
