// runner-plugins: Plugin implementations for the GitHub Actions Runner.
// This crate maps the C# `Runner.Plugins` project and provides artifact
// upload/download plugins as well as a (minimal) repository module.

pub mod artifact;
pub mod repository;

// Re-exports for convenient access
pub use artifact::download_artifact::DownloadArtifactPlugin;
pub use artifact::file_container_server::FileContainerServer;
pub use artifact::pipelines_server::PipelinesServer;
pub use artifact::publish_artifact::PublishArtifactPlugin;
