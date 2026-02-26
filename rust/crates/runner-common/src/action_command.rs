// ActionCommand mapping `ActionCommand.cs`.
// Parses workflow commands in both v1 (`##[command]data`) and v2 (`::command key=val::data`) formats.

use std::collections::HashMap;

/// A parsed workflow / action command.
#[derive(Debug, Clone)]
pub struct ActionCommand {
    /// The command name (e.g. "error", "set-output", "add-mask").
    pub command: String,
    /// Arbitrary key-value properties attached to the command.
    pub properties: HashMap<String, String>,
    /// The command data / body text.
    pub data: String,
}

/// The v1 command prefix (`##[`).
pub const V1_PREFIX: &str = "##[";

/// The v2 command delimiter (`::`) used both as prefix and as separator.
pub const V2_COMMAND_KEY: &str = "::";

// ---------------------------------------------------------------------------
// Escape mappings
// ---------------------------------------------------------------------------

struct EscapeMapping {
    token: &'static str,
    replacement: &'static str,
}

/// General escape mappings (used by v1 `Unescape`).
const ESCAPE_MAPPINGS: &[EscapeMapping] = &[
    EscapeMapping { token: ";",  replacement: "%3B" },
    EscapeMapping { token: "\r", replacement: "%0D" },
    EscapeMapping { token: "\n", replacement: "%0A" },
    EscapeMapping { token: "]",  replacement: "%5D" },
    EscapeMapping { token: "%",  replacement: "%25" },
];

/// Data escape mappings (used by v2 `UnescapeData`).
const ESCAPE_DATA_MAPPINGS: &[EscapeMapping] = &[
    EscapeMapping { token: "\r", replacement: "%0D" },
    EscapeMapping { token: "\n", replacement: "%0A" },
    EscapeMapping { token: "%",  replacement: "%25" },
];

/// Property escape mappings (used by v2 `UnescapeProperty`).
const ESCAPE_PROPERTY_MAPPINGS: &[EscapeMapping] = &[
    EscapeMapping { token: "\r", replacement: "%0D" },
    EscapeMapping { token: "\n", replacement: "%0A" },
    EscapeMapping { token: ":",  replacement: "%3A" },
    EscapeMapping { token: ",",  replacement: "%2C" },
    EscapeMapping { token: "%",  replacement: "%25" },
];

impl ActionCommand {
    /// Create a new `ActionCommand` with the given command name.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            properties: HashMap::new(),
            data: String::new(),
        }
    }

    // -----------------------------------------------------------------------
    // V2 parsing: `::command key=val,key2=val2::data`
    // -----------------------------------------------------------------------

    /// Try to parse a v2 command from a message line.
    ///
    /// The format is: `::command-name key=value,key2=value2::body data`
    ///
    /// `registered_commands` is the set of command names that are recognised.
    /// If the parsed command name is not in the set, `None` is returned.
    pub fn try_parse_v2(
        message: &str,
        registered_commands: &std::collections::HashSet<String>,
    ) -> Option<ActionCommand> {
        if message.is_empty() {
            return None;
        }

        let message = message.trim_start();
        if !message.starts_with(V2_COMMAND_KEY) {
            return None;
        }

        // Find the second `::` that separates command info from data.
        let after_prefix = &message[V2_COMMAND_KEY.len()..];
        let end_index = after_prefix.find(V2_COMMAND_KEY)?;

        let cmd_info = &after_prefix[..end_index];

        // Split command name from properties at the first space.
        let (command_name, properties_str) = match cmd_info.find(' ') {
            Some(space_idx) => (&cmd_info[..space_idx], Some(cmd_info[space_idx + 1..].trim())),
            None => (cmd_info, None),
        };

        if !registered_commands.contains(command_name) {
            return None;
        }

        let mut command = ActionCommand::new(command_name);

        // Parse properties: `key=value,key2=value2`
        if let Some(props_str) = properties_str {
            if !props_str.is_empty() {
                for prop_entry in props_str.split(',') {
                    let prop_entry = prop_entry.trim();
                    if prop_entry.is_empty() {
                        continue;
                    }
                    if let Some(eq_idx) = prop_entry.find('=') {
                        let key = &prop_entry[..eq_idx];
                        let value = &prop_entry[eq_idx + 1..];
                        if !key.is_empty() && !value.is_empty() {
                            command
                                .properties
                                .insert(key.to_string(), unescape_property(value));
                        }
                    }
                }
            }
        }

        // Data is everything after the second `::`
        let data_start = V2_COMMAND_KEY.len() + end_index + V2_COMMAND_KEY.len();
        command.data = unescape_data(&message[data_start..]);

        Some(command)
    }

    // -----------------------------------------------------------------------
    // V1 parsing: `##[command key=val;key2=val2]data`
    // -----------------------------------------------------------------------

    /// Try to parse a v1 command from a message line.
    ///
    /// The format is: `##[command-name key=value;key2=value2]body data`
    pub fn try_parse_v1(
        message: &str,
        registered_commands: &std::collections::HashSet<String>,
    ) -> Option<ActionCommand> {
        if message.is_empty() {
            return None;
        }

        let prefix_index = message.find(V1_PREFIX)?;

        // Find the closing `]`
        let rb_index = message[prefix_index..].find(']')?;
        let rb_index = prefix_index + rb_index;

        let cmd_start = prefix_index + V1_PREFIX.len();
        let cmd_info = &message[cmd_start..rb_index];

        // Split command name from properties at the first space.
        let (command_name, properties_str) = match cmd_info.find(' ') {
            Some(space_idx) => (&cmd_info[..space_idx], Some(&cmd_info[space_idx + 1..])),
            None => (cmd_info, None),
        };

        if !registered_commands.contains(command_name) {
            return None;
        }

        let mut command = ActionCommand::new(command_name);

        // Parse properties: `key=value;key2=value2`
        if let Some(props_str) = properties_str {
            for prop_entry in props_str.split(';') {
                let prop_entry = prop_entry.trim();
                if prop_entry.is_empty() {
                    continue;
                }
                if let Some(eq_idx) = prop_entry.find('=') {
                    let key = &prop_entry[..eq_idx];
                    let value = &prop_entry[eq_idx + 1..];
                    if !key.is_empty() && !value.is_empty() {
                        command
                            .properties
                            .insert(key.to_string(), unescape(value));
                    }
                }
            }
        }

        // Data is everything after the closing `]`
        command.data = unescape(&message[rb_index + 1..]);

        Some(command)
    }

    /// Escape a value using the standard escape mappings (reverse order to avoid double-encoding).
    pub fn escape_value(value: &str) -> String {
        if value.is_empty() {
            return value.to_string();
        }
        let mut escaped = value.to_string();
        // Iterate in reverse so that `%` is escaped first
        for mapping in ESCAPE_MAPPINGS.iter().rev() {
            escaped = escaped.replace(mapping.token, mapping.replacement);
        }
        escaped
    }
}

// ---------------------------------------------------------------------------
// Private unescape helpers
// ---------------------------------------------------------------------------

/// Unescape using the general escape mappings (v1 style).
fn unescape(escaped: &str) -> String {
    if escaped.is_empty() {
        return String::new();
    }
    let mut result = escaped.to_string();
    for mapping in ESCAPE_MAPPINGS {
        result = result.replace(mapping.replacement, mapping.token);
    }
    result
}

/// Unescape property values (v2 style).
fn unescape_property(escaped: &str) -> String {
    if escaped.is_empty() {
        return String::new();
    }
    let mut result = escaped.to_string();
    for mapping in ESCAPE_PROPERTY_MAPPINGS {
        result = result.replace(mapping.replacement, mapping.token);
    }
    result
}

/// Unescape command data (v2 style).
fn unescape_data(escaped: &str) -> String {
    if escaped.is_empty() {
        return String::new();
    }
    let mut result = escaped.to_string();
    for mapping in ESCAPE_DATA_MAPPINGS {
        result = result.replace(mapping.replacement, mapping.token);
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn make_commands(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_parse_v2_simple() {
        let cmds = make_commands(&["error"]);
        let result = ActionCommand::try_parse_v2("::error::something went wrong", &cmds);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, "error");
        assert_eq!(cmd.data, "something went wrong");
        assert!(cmd.properties.is_empty());
    }

    #[test]
    fn test_parse_v2_with_properties() {
        let cmds = make_commands(&["error"]);
        let result =
            ActionCommand::try_parse_v2("::error file=app.js,line=10::something wrong", &cmds);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, "error");
        assert_eq!(cmd.data, "something wrong");
        assert_eq!(cmd.properties.get("file").map(|s| s.as_str()), Some("app.js"));
        assert_eq!(cmd.properties.get("line").map(|s| s.as_str()), Some("10"));
    }

    #[test]
    fn test_parse_v2_unregistered_command() {
        let cmds = make_commands(&["warning"]);
        let result = ActionCommand::try_parse_v2("::error::data", &cmds);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_v2_unescape_data() {
        let cmds = make_commands(&["error"]);
        let result = ActionCommand::try_parse_v2("::error::line1%0Aline2%0D%25done", &cmds);
        let cmd = result.unwrap();
        assert_eq!(cmd.data, "line1\nline2\r%done");
    }

    #[test]
    fn test_parse_v1_simple() {
        let cmds = make_commands(&["warning"]);
        let result = ActionCommand::try_parse_v1("##[warning]this is a warning", &cmds);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, "warning");
        assert_eq!(cmd.data, "this is a warning");
    }

    #[test]
    fn test_parse_v1_with_properties() {
        let cmds = make_commands(&["error"]);
        let result = ActionCommand::try_parse_v1("##[error file=test.js;line=5]failure", &cmds);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, "error");
        assert_eq!(cmd.data, "failure");
        assert_eq!(cmd.properties.get("file").map(|s| s.as_str()), Some("test.js"));
        assert_eq!(cmd.properties.get("line").map(|s| s.as_str()), Some("5"));
    }

    #[test]
    fn test_parse_v1_unescape() {
        let cmds = make_commands(&["error"]);
        let result = ActionCommand::try_parse_v1("##[error]line1%0Aline2%3B%25%5D", &cmds);
        let cmd = result.unwrap();
        assert_eq!(cmd.data, "line1\nline2;%]");
    }

    #[test]
    fn test_escape_value() {
        let escaped = ActionCommand::escape_value("hello;world\n%");
        assert!(escaped.contains("%3B"));
        assert!(escaped.contains("%0A"));
        assert!(escaped.contains("%25"));
    }

    #[test]
    fn test_empty_message() {
        let cmds = make_commands(&["error"]);
        assert!(ActionCommand::try_parse_v1("", &cmds).is_none());
        assert!(ActionCommand::try_parse_v2("", &cmds).is_none());
    }
}
