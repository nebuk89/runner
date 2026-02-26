// ActionManifestManager mapping `ActionManifestManager.cs`.
// Parses action.yml / action.yaml files and provides the ActionDefinition data model.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use runner_common::constants;

/// Parsed action definition from action.yml / action.yaml.
#[derive(Debug, Clone)]
pub struct ActionDefinition {
    /// Action name.
    pub name: String,

    /// Action description.
    pub description: String,

    /// Action author.
    pub author: String,

    /// Input definitions: name → default value.
    pub inputs: HashMap<String, String>,

    /// Output definitions: name → description.
    pub outputs: HashMap<String, String>,

    /// The `runs` configuration.
    pub runs: RunsConfiguration,

    /// Steps for composite actions.
    pub steps: Vec<ActionStepDefinition>,
}

/// The `runs:` section of an action.yml.
#[derive(Debug, Clone)]
pub struct RunsConfiguration {
    /// Using: "node20", "docker", "composite", etc.
    pub using: String,

    /// Main entry point (e.g. "dist/index.js").
    pub main: Option<String>,

    /// Pre entry point.
    pub pre: Option<String>,

    /// Post entry point.
    pub post: Option<String>,

    /// Pre-if condition.
    pub pre_if: Option<String>,

    /// Post-if condition.
    pub post_if: Option<String>,

    /// Docker image (for container actions).
    pub image: Option<String>,

    /// Dockerfile (for container actions).
    pub dockerfile: Option<String>,

    /// Docker entrypoint.
    pub entrypoint: Option<String>,

    /// Docker args.
    pub args: Vec<String>,

    /// Environment for docker actions.
    pub env: HashMap<String, String>,
}

/// A step definition within a composite action.
#[derive(Debug, Clone)]
pub struct ActionStepDefinition {
    /// Step ID.
    pub id: Option<String>,

    /// Step name.
    pub name: Option<String>,

    /// Condition expression.
    pub condition: Option<String>,

    /// Uses reference (for nested actions).
    pub uses: Option<String>,

    /// Inline script (for run steps).
    pub run: Option<String>,

    /// Shell for run steps.
    pub shell: Option<String>,

    /// Working directory.
    pub working_directory: Option<String>,

    /// With inputs.
    pub with: HashMap<String, String>,

    /// Environment variables.
    pub env: Option<HashMap<String, String>>,

    /// Continue on error.
    pub continue_on_error: Option<bool>,

    /// Timeout in minutes.
    pub timeout_in_minutes: Option<u32>,
}

/// Input definition from action.yml.
#[derive(Debug, Clone, serde::Deserialize)]
struct InputDef {
    description: Option<String>,
    required: Option<bool>,
    default: Option<String>,
    deprecation_message: Option<String>,
}

/// Output definition from action.yml.
#[derive(Debug, Clone, serde::Deserialize)]
struct OutputDef {
    description: Option<String>,
    value: Option<String>,
}

/// Manages loading and parsing of action manifest files.
pub struct ActionManifestManager;

impl ActionManifestManager {
    /// Load an action definition from a directory.
    ///
    /// Tries `action.yml` first, then `action.yaml`.
    pub fn load_action(action_directory: &str) -> Result<ActionDefinition> {
        let dir = Path::new(action_directory);

        let manifest_path = dir.join(constants::path::ACTION_MANIFEST_YML_FILE);
        let yaml_path = dir.join(constants::path::ACTION_MANIFEST_YAML_FILE);

        let content = if manifest_path.exists() {
            std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("Failed to read {:?}", manifest_path))?
        } else if yaml_path.exists() {
            std::fs::read_to_string(&yaml_path)
                .with_context(|| format!("Failed to read {:?}", yaml_path))?
        } else {
            anyhow::bail!(
                "No action manifest found in {}. Expected action.yml or action.yaml.",
                action_directory
            );
        };

        Self::parse_action_yaml(&content)
    }

    /// Parse an action.yml/yaml string into an ActionDefinition.
    pub fn parse_action_yaml(content: &str) -> Result<ActionDefinition> {
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(content).context("Failed to parse action YAML")?;

        let name = yaml
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Action")
            .to_string();

        let description = yaml
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let author = yaml
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Parse inputs
        let mut inputs = HashMap::new();
        if let Some(inputs_map) = yaml.get("inputs").and_then(|v| v.as_mapping()) {
            for (key, value) in inputs_map {
                let name = key.as_str().unwrap_or("").to_string();
                let default = value
                    .get("default")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                inputs.insert(name, default);
            }
        }

        // Parse outputs
        let mut outputs = HashMap::new();
        if let Some(outputs_map) = yaml.get("outputs").and_then(|v| v.as_mapping()) {
            for (key, value) in outputs_map {
                let name = key.as_str().unwrap_or("").to_string();
                let desc = value
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                outputs.insert(name, desc);
            }
        }

        // Parse runs
        let runs_yaml = yaml
            .get("runs")
            .ok_or_else(|| anyhow::anyhow!("Missing 'runs' section in action manifest"))?;

        let using = runs_yaml
            .get("using")
            .and_then(|v| v.as_str())
            .unwrap_or("node20")
            .to_string();

        let main = runs_yaml
            .get("main")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let pre = runs_yaml
            .get("pre")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let post = runs_yaml
            .get("post")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let pre_if = runs_yaml
            .get("pre-if")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let post_if = runs_yaml
            .get("post-if")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let image = runs_yaml
            .get("image")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let dockerfile = runs_yaml
            .get("dockerfile")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let entrypoint = runs_yaml
            .get("entrypoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let args = runs_yaml
            .get("args")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let runs_env = parse_string_map(runs_yaml.get("env"));

        let runs = RunsConfiguration {
            using: using.clone(),
            main,
            pre,
            post,
            pre_if,
            post_if,
            image,
            dockerfile,
            entrypoint,
            args,
            env: runs_env,
        };

        // Parse composite steps
        let steps = if using == "composite" {
            Self::parse_composite_steps(runs_yaml)?
        } else {
            Vec::new()
        };

        Ok(ActionDefinition {
            name,
            description,
            author,
            inputs,
            outputs,
            runs,
            steps,
        })
    }

    /// Parse the `steps` array in a composite action's `runs` section.
    fn parse_composite_steps(runs_yaml: &serde_yaml::Value) -> Result<Vec<ActionStepDefinition>> {
        let steps_yaml = match runs_yaml.get("steps").and_then(|v| v.as_sequence()) {
            Some(seq) => seq,
            None => return Ok(Vec::new()),
        };

        let mut steps = Vec::new();

        for (i, step_yaml) in steps_yaml.iter().enumerate() {
            let step = ActionStepDefinition {
                id: step_yaml
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                name: step_yaml
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                condition: step_yaml
                    .get("if")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                uses: step_yaml
                    .get("uses")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                run: step_yaml
                    .get("run")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                shell: step_yaml
                    .get("shell")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                working_directory: step_yaml
                    .get("working-directory")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                with: parse_string_map(step_yaml.get("with")),
                env: step_yaml.get("env").map(|v| parse_string_map(Some(v))),
                continue_on_error: step_yaml
                    .get("continue-on-error")
                    .and_then(|v| v.as_bool()),
                timeout_in_minutes: step_yaml
                    .get("timeout-minutes")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
            };

            steps.push(step);
        }

        Ok(steps)
    }
}

/// Parse a YAML mapping into a HashMap<String, String>.
fn parse_string_map(value: Option<&serde_yaml::Value>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(mapping) = value.and_then(|v| v.as_mapping()) {
        for (k, v) in mapping {
            let key = k.as_str().unwrap_or("").to_string();
            let val = match v {
                serde_yaml::Value::String(s) => s.clone(),
                serde_yaml::Value::Bool(b) => b.to_string(),
                serde_yaml::Value::Number(n) => n.to_string(),
                _ => v.as_str().unwrap_or("").to_string(),
            };
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_node_action() {
        let yaml = r#"
name: 'My Action'
description: 'A test action'
inputs:
  name:
    description: 'Name input'
    required: true
    default: 'world'
outputs:
  result:
    description: 'The result'
runs:
  using: 'node20'
  main: 'dist/index.js'
  pre: 'dist/setup.js'
  post: 'dist/cleanup.js'
"#;

        let def = ActionManifestManager::parse_action_yaml(yaml).unwrap();
        assert_eq!(def.name, "My Action");
        assert_eq!(def.runs.using, "node20");
        assert_eq!(def.runs.main, Some("dist/index.js".to_string()));
        assert_eq!(def.runs.pre, Some("dist/setup.js".to_string()));
        assert_eq!(def.runs.post, Some("dist/cleanup.js".to_string()));
        assert_eq!(def.inputs.get("name"), Some(&"world".to_string()));
        assert!(def.outputs.contains_key("result"));
        assert!(def.steps.is_empty());
    }

    #[test]
    fn test_parse_composite_action() {
        let yaml = r#"
name: 'Composite Action'
description: 'A composite action'
inputs:
  who-to-greet:
    description: 'Who to greet'
    default: 'World'
outputs:
  greeting:
    description: 'The greeting'
runs:
  using: 'composite'
  steps:
    - id: greet
      run: echo "Hello ${{ inputs.who-to-greet }}"
      shell: bash
    - uses: actions/checkout@v4
"#;

        let def = ActionManifestManager::parse_action_yaml(yaml).unwrap();
        assert_eq!(def.name, "Composite Action");
        assert_eq!(def.runs.using, "composite");
        assert_eq!(def.steps.len(), 2);
        assert_eq!(def.steps[0].id, Some("greet".to_string()));
        assert!(def.steps[0].run.is_some());
        assert_eq!(def.steps[0].shell, Some("bash".to_string()));
        assert_eq!(def.steps[1].uses, Some("actions/checkout@v4".to_string()));
    }

    #[test]
    fn test_parse_docker_action() {
        let yaml = r#"
name: 'Docker Action'
description: 'A docker action'
runs:
  using: 'docker'
  image: 'Dockerfile'
  entrypoint: '/entrypoint.sh'
  args:
    - '--flag'
    - 'value'
  env:
    MY_VAR: hello
"#;

        let def = ActionManifestManager::parse_action_yaml(yaml).unwrap();
        assert_eq!(def.runs.using, "docker");
        assert_eq!(def.runs.image, Some("Dockerfile".to_string()));
        assert_eq!(def.runs.entrypoint, Some("/entrypoint.sh".to_string()));
        assert_eq!(def.runs.args.len(), 2);
        assert_eq!(def.runs.env.get("MY_VAR"), Some(&"hello".to_string()));
    }
}
