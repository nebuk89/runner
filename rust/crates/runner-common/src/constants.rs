// Constants mapping `Constants.cs` from the C# runner.
// This module contains ALL enums, constants, and nested constant groups.

use std::fmt;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Well-known directories used by the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WellKnownDirectory {
    Bin,
    Diag,
    Externals,
    Root,
    Actions,
    Temp,
    Tools,
    Update,
    Work,
}

impl fmt::Display for WellKnownDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Well-known configuration files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WellKnownConfigFile {
    Runner,
    MigratedRunner,
    Credentials,
    MigratedCredentials,
    RSACredentials,
    Service,
    CredentialStore,
    Certificates,
    Options,
    SetupInfo,
    Telemetry,
}

impl fmt::Display for WellKnownConfigFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Operating system platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsPlatform {
    Linux,
    MacOS,
    Windows,
}

impl fmt::Display for OsPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OsPlatform::Linux => write!(f, "Linux"),
            OsPlatform::MacOS => write!(f, "OSX"),
            OsPlatform::Windows => write!(f, "Windows"),
        }
    }
}

impl OsPlatform {
    /// Label name for runner registration, matching C# VarUtil.OS.
    /// Note: Display returns "OSX" (internal platform name), but the runner
    /// label must be "macOS" to match GitHub's `runs-on` expectations.
    pub fn label_name(&self) -> &'static str {
        match self {
            OsPlatform::Linux => "Linux",
            OsPlatform::MacOS => "macOS",
            OsPlatform::Windows => "Windows",
        }
    }
}

/// CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Architecture {
    X86,
    X64,
    Arm,
    Arm64,
}

impl fmt::Display for Architecture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Architecture::X86 => write!(f, "X86"),
            Architecture::X64 => write!(f, "X64"),
            Architecture::Arm => write!(f, "ARM"),
            Architecture::Arm64 => write!(f, "ARM64"),
        }
    }
}

impl Architecture {
    /// Label name for runner registration, matching C# VarUtil.OSArchitecture.
    pub fn label_name(&self) -> &'static str {
        match self {
            Architecture::X86 => "X86",
            Architecture::X64 => "X64",
            Architecture::Arm => "ARM",
            Architecture::Arm64 => "ARM64",
        }
    }
}

// ---------------------------------------------------------------------------
// Platform detection (compile-time)
// ---------------------------------------------------------------------------

/// The current OS platform, detected at compile time.
#[cfg(target_os = "linux")]
pub const CURRENT_PLATFORM: OsPlatform = OsPlatform::Linux;
#[cfg(target_os = "macos")]
pub const CURRENT_PLATFORM: OsPlatform = OsPlatform::MacOS;
#[cfg(target_os = "windows")]
pub const CURRENT_PLATFORM: OsPlatform = OsPlatform::Windows;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub const CURRENT_PLATFORM: OsPlatform = OsPlatform::Linux; // default fallback

/// The current CPU architecture, detected at compile time.
#[cfg(target_arch = "x86")]
pub const CURRENT_ARCHITECTURE: Architecture = Architecture::X86;
#[cfg(target_arch = "x86_64")]
pub const CURRENT_ARCHITECTURE: Architecture = Architecture::X64;
#[cfg(target_arch = "arm")]
pub const CURRENT_ARCHITECTURE: Architecture = Architecture::Arm;
#[cfg(target_arch = "aarch64")]
pub const CURRENT_ARCHITECTURE: Architecture = Architecture::Arm64;
#[cfg(not(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "arm",
    target_arch = "aarch64"
)))]
pub const CURRENT_ARCHITECTURE: Architecture = Architecture::X64; // default fallback

// ---------------------------------------------------------------------------
// Top-level constants
// ---------------------------------------------------------------------------

/// Path environment variable name (platform-specific).
#[cfg(target_os = "windows")]
pub const PATH_VARIABLE: &str = "Path";
#[cfg(not(target_os = "windows"))]
pub const PATH_VARIABLE: &str = "PATH";

/// Environment variable used to track runner process lineage.
pub const PROCESS_TRACKING_ID: &str = "RUNNER_TRACKING_ID";

/// Prefix for plugin trace output lines.
pub const PLUGIN_TRACE_PREFIX: &str = "##[plugin.trace]";

/// Maximum retry attempts for runner download.
pub const RUNNER_DOWNLOAD_RETRY_MAX_ATTEMPTS: u32 = 3;

/// Maximum depth for composite actions.
pub const COMPOSITE_ACTIONS_MAX_DEPTH: u32 = 9;

/// Timeout before force-exiting on unload.
pub const EXIT_ON_UNLOAD_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// CommandLine
// ---------------------------------------------------------------------------

/// Command-line argument names.
pub mod command_line {
    /// Named arguments (key=value style).
    pub mod args {
        pub const AUTH: &str = "auth";
        pub const LABELS: &str = "labels";
        pub const MONITOR_SOCKET_ADDRESS: &str = "monitorsocketaddress";
        pub const NAME: &str = "name";
        pub const RUNNER_GROUP: &str = "runnergroup";
        pub const STARTUP_TYPE: &str = "startuptype";
        pub const URL: &str = "url";
        pub const USER_NAME: &str = "username";
        pub const WINDOWS_LOGON_ACCOUNT: &str = "windowslogonaccount";
        pub const WORK: &str = "work";
        pub const TOKEN: &str = "token";
        pub const PAT: &str = "pat";
        pub const WINDOWS_LOGON_PASSWORD: &str = "windowslogonpassword";
        pub const JIT_CONFIG: &str = "jitconfig";

        /// Returns the list of arguments that contain secret values.
        pub fn secrets() -> &'static [&'static str] {
            &[PAT, TOKEN, WINDOWS_LOGON_PASSWORD, JIT_CONFIG]
        }
    }

    /// Top-level commands.
    pub mod commands {
        pub const CONFIGURE: &str = "configure";
        pub const REMOVE: &str = "remove";
        pub const RUN: &str = "run";
        pub const WARMUP: &str = "warmup";
    }

    /// Boolean flags.
    pub mod flags {
        pub const CHECK: &str = "check";
        pub const COMMIT: &str = "commit";
        pub const EPHEMERAL: &str = "ephemeral";
        pub const GENERATE_SERVICE_CONFIG: &str = "generateServiceConfig";
        pub const HELP: &str = "help";
        pub const LOCAL: &str = "local";
        pub const NO_DEFAULT_LABELS: &str = "no-default-labels";
        pub const REPLACE: &str = "replace";
        pub const DISABLE_UPDATE: &str = "disableupdate";
        pub const ONCE: &str = "once";
        pub const RUN_AS_SERVICE: &str = "runasservice";
        pub const UNATTENDED: &str = "unattended";
        pub const VERSION: &str = "version";
    }
}

// ---------------------------------------------------------------------------
// ReturnCode
// ---------------------------------------------------------------------------

/// Process return / exit codes.
pub mod return_code {
    pub const SUCCESS: i32 = 0;
    pub const TERMINATED_ERROR: i32 = 1;
    pub const RETRYABLE_ERROR: i32 = 2;
    pub const RUNNER_UPDATING: i32 = 3;
    pub const RUN_ONCE_RUNNER_UPDATING: i32 = 4;
    pub const SESSION_CONFLICT: i32 = 5;
    pub const RUNNER_CONFIGURATION_REFRESHED: i32 = 6;
}

// ---------------------------------------------------------------------------
// Features
// ---------------------------------------------------------------------------

/// Feature flag string constants (matching the C# `Features` class).
pub mod features {
    pub const DISK_SPACE_WARNING: &str = "runner.diskspace.warning";
    pub const LOG_TEMPLATE_ERRORS_AS_DEBUG_MESSAGES: &str =
        "DistributedTask.LogTemplateErrorsAsDebugMessages";
    pub const USE_CONTAINER_PATH_FOR_TEMPLATE: &str =
        "DistributedTask.UseContainerPathForTemplate";
    pub const ALLOW_RUNNER_CONTAINER_HOOKS: &str = "DistributedTask.AllowRunnerContainerHooks";
    pub const ADD_CHECK_RUN_ID_TO_JOB_CONTEXT: &str = "actions_add_check_run_id_to_job_context";
    pub const DISPLAY_HELPFUL_ACTIONS_DOWNLOAD_ERRORS: &str =
        "actions_display_helpful_actions_download_errors";
    pub const SNAPSHOT_PREFLIGHT_HOSTED_RUNNER_CHECK: &str =
        "actions_snapshot_preflight_hosted_runner_check";
    pub const SNAPSHOT_PREFLIGHT_IMAGE_GEN_POOL_CHECK: &str =
        "actions_snapshot_preflight_image_gen_pool_check";
    pub const COMPARE_WORKFLOW_PARSER: &str = "actions_runner_compare_workflow_parser";
    pub const SET_ORCHESTRATION_ID_ENV_FOR_ACTIONS: &str =
        "actions_set_orchestration_id_env_for_actions";
    pub const SEND_JOB_LEVEL_ANNOTATIONS: &str = "actions_send_job_level_annotations";
    pub const EMIT_COMPOSITE_MARKERS: &str = "actions_runner_emit_composite_markers";
}

// ---------------------------------------------------------------------------
// NodeMigration
// ---------------------------------------------------------------------------

/// Node version migration constants.
pub mod node_migration {
    pub const NODE20: &str = "node20";
    pub const NODE24: &str = "node24";

    pub const FORCE_NODE24_VARIABLE: &str = "FORCE_JAVASCRIPT_ACTIONS_TO_NODE24";
    pub const ALLOW_UNSECURE_NODE_VERSION_VARIABLE: &str =
        "ACTIONS_ALLOW_USE_UNSECURE_NODE_VERSION";

    pub const USE_NODE24_BY_DEFAULT_FLAG: &str = "actions.runner.usenode24bydefault";
    pub const REQUIRE_NODE24_FLAG: &str = "actions.runner.requirenode24";
    pub const WARN_ON_NODE20_FLAG: &str = "actions.runner.warnonnode20";

    pub const NODE20_DEPRECATION_URL: &str =
        "https://github.blog/changelog/2025-09-19-deprecation-of-node-20-on-github-actions-runners/";
}

// ---------------------------------------------------------------------------
// Internal telemetry / runner events
// ---------------------------------------------------------------------------

pub const INTERNAL_TELEMETRY_ISSUE_DATA_KEY: &str = "_internal_telemetry";
pub const TELEMETRY_RECORD_ID: &str = "11111111-1111-1111-1111-111111111111";
pub const WORKER_CRASH: &str = "WORKER_CRASH";
pub const LOW_DISK_SPACE: &str = "LOW_DISK_SPACE";
pub const UNSUPPORTED_COMMAND: &str = "UNSUPPORTED_COMMAND";
pub const RESULTS_UPLOAD_FAILURE: &str = "RESULTS_UPLOAD_FAILURE";

pub const UNSUPPORTED_COMMAND_MESSAGE: &str = "The `{0}` command is deprecated and will be disabled soon. Please upgrade to using Environment Files. For more information see: https://github.blog/changelog/2022-10-11-github-actions-deprecating-save-state-and-set-output-commands/";
pub const UNSUPPORTED_COMMAND_MESSAGE_DISABLED: &str = "The `{0}` command is disabled. Please upgrade to using Environment Files or opt into unsecure command execution by setting the `ACTIONS_ALLOW_UNSECURE_COMMANDS` environment variable to `true`. For more information see: https://github.blog/changelog/2020-10-01-github-actions-deprecating-set-env-and-add-path-commands/";
pub const UNSUPPORTED_STOP_COMMAND_TOKEN_DISABLED: &str = "You cannot use a endToken that is an empty string, the string 'pause-logging', or another workflow command. For more information see: https://docs.github.com/actions/learn-github-actions/workflow-commands-for-github-actions#example-stopping-and-starting-workflow-commands or opt into insecure command execution by setting the `ACTIONS_ALLOW_UNSECURE_STOPCOMMAND_TOKENS` environment variable to `true`.";
pub const UNSUPPORTED_SUMMARY_SIZE: &str = "$GITHUB_STEP_SUMMARY upload aborted, supports content up to a size of {0}k, got {1}k. For more information see: https://docs.github.com/actions/using-workflows/workflow-commands-for-github-actions#adding-a-markdown-summary";
pub const SUMMARY_UPLOAD_ERROR: &str = "$GITHUB_STEP_SUMMARY upload aborted, an error occurred when uploading the summary. For more information see: https://docs.github.com/actions/using-workflows/workflow-commands-for-github-actions#adding-a-markdown-summary";

// ---------------------------------------------------------------------------
// RunnerEvent
// ---------------------------------------------------------------------------

pub mod runner_event {
    pub const REGISTER: &str = "register";
    pub const REMOVE: &str = "remove";
}

// ---------------------------------------------------------------------------
// Pipeline::Path
// ---------------------------------------------------------------------------

pub mod pipeline {
    pub mod path {
        pub const PIPELINE_MAPPING_DIRECTORY: &str = "_PipelineMapping";
        pub const TRACKING_CONFIG_FILE: &str = "PipelineFolder.json";
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

pub mod configuration {
    pub const OAUTH_ACCESS_TOKEN: &str = "OAuthAccessToken";
    pub const OAUTH: &str = "OAuth";
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

pub mod expressions {
    pub const ALWAYS: &str = "always";
    pub const CANCELLED: &str = "cancelled";
    pub const FAILURE: &str = "failure";
    pub const SUCCESS: &str = "success";
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

pub mod hooks {
    pub const JOB_STARTED_STEP_NAME: &str = "Set up runner";
    pub const JOB_COMPLETED_STEP_NAME: &str = "Complete runner";
    pub const CONTAINER_HOOKS_PATH: &str = "ACTIONS_RUNNER_CONTAINER_HOOKS";
}

// ---------------------------------------------------------------------------
// Path constants
// ---------------------------------------------------------------------------

pub mod path {
    pub const ACTIONS_DIRECTORY: &str = "_actions";
    pub const ACTION_MANIFEST_YML_FILE: &str = "action.yml";
    pub const ACTION_MANIFEST_YAML_FILE: &str = "action.yaml";
    pub const BIN_DIRECTORY: &str = "bin";
    pub const DIAG_DIRECTORY: &str = "_diag";
    pub const EXTERNALS_DIRECTORY: &str = "externals";
    pub const RUNNER_DIAGNOSTIC_LOG_PREFIX: &str = "Runner_";
    pub const TEMP_DIRECTORY: &str = "_temp";
    pub const TOOL_DIRECTORY: &str = "_tool";
    pub const UPDATE_DIRECTORY: &str = "_update";
    pub const WORK_DIRECTORY: &str = "_work";
    pub const WORKER_DIAGNOSTIC_LOG_PREFIX: &str = "Worker_";
}

// ---------------------------------------------------------------------------
// Variables
// ---------------------------------------------------------------------------

pub mod variables {
    pub const MACRO_PREFIX: &str = "$(";
    pub const MACRO_SUFFIX: &str = ")";

    pub mod actions {
        pub const ALLOW_UNSUPPORTED_COMMANDS: &str = "ACTIONS_ALLOW_UNSECURE_COMMANDS";
        pub const ALLOW_UNSUPPORTED_STOP_COMMAND_TOKENS: &str =
            "ACTIONS_ALLOW_UNSECURE_STOPCOMMAND_TOKENS";
        pub const REQUIRE_JOB_CONTAINER: &str = "ACTIONS_RUNNER_REQUIRE_JOB_CONTAINER";
        pub const RUNNER_DEBUG: &str = "ACTIONS_RUNNER_DEBUG";
        pub const STEP_DEBUG: &str = "ACTIONS_STEP_DEBUG";
    }

    pub mod agent {
        pub const TOOLS_DIRECTORY: &str = "agent.ToolsDirectory";
        pub const FORCED_INTERNAL_NODE_VERSION: &str =
            "ACTIONS_RUNNER_FORCED_INTERNAL_NODE_VERSION";
        pub const FORCED_ACTIONS_NODE_VERSION: &str = "ACTIONS_RUNNER_FORCE_ACTIONS_NODE_VERSION";
        pub const PRINT_LOG_TO_STDOUT: &str = "ACTIONS_RUNNER_PRINT_LOG_TO_STDOUT";
        pub const ACTION_ARCHIVE_CACHE_DIRECTORY: &str = "ACTIONS_RUNNER_ACTION_ARCHIVE_CACHE";
        pub const SYMLINK_CACHED_ACTIONS: &str = "ACTIONS_RUNNER_SYMLINK_CACHED_ACTIONS";
        pub const EMIT_COMPOSITE_MARKERS: &str = "ACTIONS_RUNNER_EMIT_COMPOSITE_MARKERS";
    }

    pub mod system {
        pub const ACCESS_TOKEN: &str = "system.accessToken";
        pub const CULTURE: &str = "system.culture";
        pub const PHASE_DISPLAY_NAME: &str = "system.phaseDisplayName";
        pub const JOB_REQUEST_TYPE: &str = "system.jobRequestType";
        pub const ORCHESTRATION_ID: &str = "system.orchestrationId";
    }
}

// ---------------------------------------------------------------------------
// OperatingSystem
// ---------------------------------------------------------------------------

pub mod operating_system {
    pub const WINDOWS_11_BUILD_VERSION: u32 = 22000;
    pub const WINDOWS_11_MAJOR_VERSION: u32 = 10;
}
