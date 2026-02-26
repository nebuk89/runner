// RunnerService trait and ServiceLocator mapping `RunnerService.cs`.

use crate::host_context::HostContext;
use std::any::Any;
use std::sync::Arc;

/// The core trait for all runner services, equivalent to the C# `IRunnerService` interface.
///
/// Every service in the runner implements this trait. Services are lazily created and
/// cached by the `HostContext` (the dependency injection container).
pub trait RunnerService: Any + Send + Sync {
    /// Initialize this service with a reference to the host context.
    ///
    /// Called exactly once when the service is first created by the `HostContext`.
    fn initialize(&mut self, context: Arc<HostContext>);

    /// Return the name used for diagnostic tracing.
    fn trace_name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// Upcast to `Any` for downcasting in the service registry.
    fn as_any(&self) -> &dyn Any;

    /// Upcast to mutable `Any` for downcasting in the service registry.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A trait that maps an interface type to its default concrete implementation.
///
/// In C# this was the `[ServiceLocator(Default = typeof(...))]` attribute.
/// In Rust we use a trait so that `HostContext` can look up the concrete type
/// to instantiate for a given trait/interface type.
pub trait ServiceLocator {
    /// The concrete type that implements this service interface.
    type Implementation: RunnerService + Default;
}

/// Startup type enum - how the runner was launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupType {
    Manual,
    Service,
    AutoStartup,
}

impl Default for StartupType {
    fn default() -> Self {
        StartupType::Manual
    }
}

/// The reason the runner is shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    UserCancelled = 0,
    OperatingSystemShutdown = 1,
}

impl std::fmt::Display for ShutdownReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShutdownReason::UserCancelled => write!(f, "UserCancelled"),
            ShutdownReason::OperatingSystemShutdown => write!(f, "OperatingSystemShutdown"),
        }
    }
}
