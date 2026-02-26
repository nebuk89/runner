// HostContext mapping `HostContext.cs`.
// THE central dependency injection container and application context.

use crate::constants::{self, WellKnownConfigFile, WellKnownDirectory};
use crate::runner_service::{ShutdownReason, StartupType};
use crate::secret_masker::SecretMasker;
use crate::tracing::{TraceSetting, TraceManager, Tracing};

use dashmap::DashMap;
use runner_sdk::{RunnerWebProxy, TraceWriter, build_constants};
use std::any::{Any, TypeId};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// The central application context and dependency injection container.
///
/// Maps `HostContext` from the C# runner. All services are lazily created
/// and cached here. Provides directory resolution, config file path lookup,
/// trace creation, and graceful shutdown coordination.
pub struct HostContext {
    /// The host type string (e.g. "Runner", "Worker").
    host_type: String,

    /// Cached service instances, keyed by `TypeId` of the interface/trait.
    service_instances: DashMap<TypeId, Arc<dyn Any + Send + Sync>>,

    /// Cancellation token for coordinated runner shutdown.
    runner_shutdown_token: CancellationToken,

    /// The reason the runner is shutting down (set once `shutdown_runner` is called).
    runner_shutdown_reason: Mutex<Option<ShutdownReason>>,

    /// Secret masker shared across the entire runner process.
    pub secret_masker: Arc<SecretMasker>,

    /// Web proxy configuration read from environment variables.
    pub web_proxy: RunnerWebProxy,

    /// User-Agent header values sent with HTTP requests.
    pub user_agents: Mutex<Vec<String>>,

    /// Startup type - how the runner was launched.
    pub startup_type: Mutex<StartupType>,

    /// Trace manager for creating per-component trace sources.
    trace_manager: TraceManager,

    /// Override for the runner root directory (used in tests).
    root_override: Mutex<Option<PathBuf>>,
}

impl HostContext {
    /// Create a new `HostContext`.
    ///
    /// `host_type` should be `"Runner"` for the listener or `"Worker"` for the worker process.
    pub fn new(host_type: impl Into<String>) -> Arc<Self> {
        let host_type = host_type.into();
        assert!(!host_type.is_empty(), "host_type must not be empty");

        let secret_masker = Arc::new(SecretMasker::new());
        let web_proxy = RunnerWebProxy::new();

        // Register proxy passwords as secrets
        if let Some(ref password) = web_proxy.http_proxy_password {
            if !password.is_empty() {
                secret_masker.add_value(password);
            }
        }
        if let Some(ref password) = web_proxy.https_proxy_password {
            if !password.is_empty() {
                secret_masker.add_value(password);
            }
        }

        // Determine print-to-stdout setting
        let print_to_stdout = env::var(constants::variables::agent::PRINT_LOG_TO_STDOUT)
            .ok()
            .and_then(|v| runner_sdk::StringUtil::convert_to_bool(&v))
            .unwrap_or(false);

        let trace_setting = TraceSetting {
            print_to_stdout,
            ..TraceSetting::default()
        };
        let trace_manager = TraceManager::with_setting(secret_masker.clone(), trace_setting);

        let default_user_agent = format!(
            "GitHubActionsRunner-{}/{}",
            build_constants::RunnerPackage::PACKAGE_NAME,
            build_constants::RunnerPackage::VERSION
        );

        Arc::new(Self {
            host_type,
            service_instances: DashMap::new(),
            runner_shutdown_token: CancellationToken::new(),
            runner_shutdown_reason: Mutex::new(None),
            secret_masker,
            web_proxy,
            user_agents: Mutex::new(vec![default_user_agent]),
            startup_type: Mutex::new(StartupType::default()),
            trace_manager,
            root_override: Mutex::new(None),
        })
    }

    // -----------------------------------------------------------------------
    // Service container
    // -----------------------------------------------------------------------

    /// Register a pre-built service instance in the container.
    ///
    /// `T` should be the trait / interface type used for lookup.
    pub fn register_service<T: Any + Send + Sync + 'static>(&self, service: Arc<T>) {
        self.service_instances
            .insert(TypeId::of::<T>(), service as Arc<dyn Any + Send + Sync>);
    }

    /// Get a cached service instance, or `None` if not yet registered.
    pub fn get_service<T: Any + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.service_instances
            .get(&TypeId::of::<T>())
            .and_then(|entry| entry.value().clone().downcast::<T>().ok())
    }

    /// Create a new service instance (does NOT cache it).
    ///
    /// For types that implement `Default` + `RunnerService`, this creates
    /// a new instance and initializes it.
    pub fn create_service_default<T: Default + Any + Send + Sync + 'static>(
        self: &Arc<Self>,
    ) -> Arc<T> {
        let service = Arc::new(T::default());
        service
    }

    /// Get or create a service. If a cached instance exists it is returned;
    /// otherwise a new `Default` instance is created, registered, and returned.
    pub fn get_or_create_service<T: Default + Any + Send + Sync + 'static>(
        self: &Arc<Self>,
    ) -> Arc<T> {
        if let Some(existing) = self.get_service::<T>() {
            return existing;
        }

        let service = Arc::new(T::default());
        self.register_service(service.clone());
        service
    }

    // -----------------------------------------------------------------------
    // Directory resolution
    // -----------------------------------------------------------------------

    /// Override the root directory (used primarily for testing).
    pub fn set_root_override(&self, path: PathBuf) {
        *self.root_override.lock().unwrap() = Some(path);
    }

    /// Resolve the path for a well-known directory.
    pub fn get_directory(&self, directory: WellKnownDirectory) -> PathBuf {
        let path = match directory {
            WellKnownDirectory::Bin => {
                // In production this is the directory containing the runner binary.
                // We use the current executable's directory as the default.
                env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."))
            }

            WellKnownDirectory::Root => {
                if let Some(ref root) = *self.root_override.lock().unwrap() {
                    return root.clone();
                }
                // Root is the parent of the Bin directory.
                let bin = self.get_directory(WellKnownDirectory::Bin);
                bin.parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| bin.clone())
            }

            WellKnownDirectory::Diag => {
                self.get_directory(WellKnownDirectory::Root)
                    .join(constants::path::DIAG_DIRECTORY)
            }

            WellKnownDirectory::Externals => {
                self.get_directory(WellKnownDirectory::Root)
                    .join(constants::path::EXTERNALS_DIRECTORY)
            }

            WellKnownDirectory::Temp => {
                self.get_directory(WellKnownDirectory::Work)
                    .join(constants::path::TEMP_DIRECTORY)
            }

            WellKnownDirectory::Actions => {
                self.get_directory(WellKnownDirectory::Work)
                    .join(constants::path::ACTIONS_DIRECTORY)
            }

            WellKnownDirectory::Tools => {
                // Check various environment variables for the tools directory
                env::var("RUNNER_TOOL_CACHE")
                    .ok()
                    .or_else(|| env::var("RUNNER_TOOLSDIRECTORY").ok())
                    .or_else(|| env::var("AGENT_TOOLSDIRECTORY").ok())
                    .or_else(|| env::var(constants::variables::agent::TOOLS_DIRECTORY).ok())
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        self.get_directory(WellKnownDirectory::Work)
                            .join(constants::path::TOOL_DIRECTORY)
                    })
            }

            WellKnownDirectory::Update => {
                self.get_directory(WellKnownDirectory::Work)
                    .join(constants::path::UPDATE_DIRECTORY)
            }

            WellKnownDirectory::Work => {
                // Default work folder; in production this comes from RunnerSettings.
                // When no settings are loaded, use "_work" under root.
                self.get_directory(WellKnownDirectory::Root)
                    .join(constants::path::WORK_DIRECTORY)
            }
        };

        path
    }

    /// Set the work folder path explicitly (used after loading settings).
    /// This stores a "Work" directory override in the service instances map.
    pub fn set_work_folder(&self, work_folder: &str) {
        let root = self.get_directory(WellKnownDirectory::Root);
        let full_path = if Path::new(work_folder).is_absolute() {
            PathBuf::from(work_folder)
        } else {
            root.join(work_folder)
        };
        // Store the resolved work path as a service
        self.service_instances.insert(
            TypeId::of::<WorkFolderOverride>(),
            Arc::new(WorkFolderOverride(full_path)) as Arc<dyn Any + Send + Sync>,
        );
    }

    /// Get the work folder if explicitly set.
    pub fn get_work_folder_override(&self) -> Option<PathBuf> {
        self.service_instances
            .get(&TypeId::of::<WorkFolderOverride>())
            .and_then(|entry| entry.value().clone().downcast::<WorkFolderOverride>().ok())
            .map(|wf| wf.0.clone())
    }

    // -----------------------------------------------------------------------
    // Config file resolution
    // -----------------------------------------------------------------------

    /// Resolve the path for a well-known configuration file.
    pub fn get_config_file(&self, config_file: WellKnownConfigFile) -> PathBuf {
        let root = self.get_directory(WellKnownDirectory::Root);
        match config_file {
            WellKnownConfigFile::Runner => root.join(".runner"),
            WellKnownConfigFile::MigratedRunner => root.join(".runner_migrated"),
            WellKnownConfigFile::Credentials => root.join(".credentials"),
            WellKnownConfigFile::MigratedCredentials => root.join(".credentials_migrated"),
            WellKnownConfigFile::RSACredentials => root.join(".credentials_rsaparams"),
            WellKnownConfigFile::Service => root.join(".service"),
            WellKnownConfigFile::CredentialStore => {
                #[cfg(target_os = "macos")]
                {
                    root.join(".credential_store.keychain")
                }
                #[cfg(not(target_os = "macos"))]
                {
                    root.join(".credential_store")
                }
            }
            WellKnownConfigFile::Certificates => root.join(".certificates"),
            WellKnownConfigFile::Options => root.join(".options"),
            WellKnownConfigFile::SetupInfo => root.join(".setup_info"),
            WellKnownConfigFile::Telemetry => {
                self.get_directory(WellKnownDirectory::Diag).join(".telemetry")
            }
        }
    }

    // -----------------------------------------------------------------------
    // Tracing
    // -----------------------------------------------------------------------

    /// Get a trace source for the given component name.
    pub fn get_trace(&self, name: &str) -> Tracing {
        self.trace_manager.get(name)
    }

    // -----------------------------------------------------------------------
    // Shutdown
    // -----------------------------------------------------------------------

    /// Get the cancellation token that is triggered on runner shutdown.
    pub fn runner_shutdown_token(&self) -> CancellationToken {
        self.runner_shutdown_token.clone()
    }

    /// Get the reason the runner is shutting down, if shutdown has been initiated.
    pub fn runner_shutdown_reason(&self) -> Option<ShutdownReason> {
        *self.runner_shutdown_reason.lock().unwrap()
    }

    /// Initiate runner shutdown with the given reason.
    pub fn shutdown_runner(&self, reason: ShutdownReason) {
        let trace = self.get_trace("HostContext");
        trace.info(&format!("Runner will be shutdown for {}", reason));
        *self.runner_shutdown_reason.lock().unwrap() = Some(reason);
        self.runner_shutdown_token.cancel();
    }

    // -----------------------------------------------------------------------
    // Misc
    // -----------------------------------------------------------------------

    /// Get the host type string.
    pub fn host_type(&self) -> &str {
        &self.host_type
    }

    /// Load default user agent strings (proxy info, commit hash, PID, etc.)
    pub fn load_default_user_agents(&self) {
        let mut agents = self.user_agents.lock().unwrap();

        // Proxy configured?
        if self.web_proxy.http_proxy_address.is_some()
            || self.web_proxy.https_proxy_address.is_some()
        {
            agents.push(format!("HttpProxyConfigured/{}", true));
        }

        // Commit hash
        agents.push(format!(
            "CommitSHA/{}",
            build_constants::Source::COMMIT_HASH
        ));

        // Extra user agent from env
        if let Ok(extra) = env::var("GITHUB_ACTIONS_RUNNER_EXTRA_USER_AGENT") {
            if !extra.is_empty() {
                agents.push(extra);
            }
        }

        // PID
        agents.push(format!("Pid/{}", std::process::id()));

        // Creation time
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.7fZ");
        agents.push(format!("CreationTime/{}", now));

        // Host type
        agents.push(format!("({})", self.host_type));
    }

    /// Async delay helper (convenience).
    pub async fn delay(
        &self,
        duration: std::time::Duration,
        cancellation_token: CancellationToken,
    ) {
        tokio::select! {
            _ = tokio::time::sleep(duration) => {}
            _ = cancellation_token.cancelled() => {}
        }
    }
}

/// Internal marker type for storing the work folder override.
struct WorkFolderOverride(PathBuf);
