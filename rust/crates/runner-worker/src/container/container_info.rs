// ContainerInfo mapping container data structures.
// Describes a Docker container (image, network, env, volumes, ports, path mappings).

use std::collections::HashMap;

/// Information about a Docker container used in a job.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    /// Docker image name (e.g. "ubuntu:22.04", "node:20").
    pub image: String,

    /// Container ID after creation (set by Docker).
    pub container_id: Option<String>,

    /// Container name.
    pub container_name: String,

    /// Docker network to attach to.
    pub network: Option<String>,

    /// Custom entrypoint override.
    pub entrypoint: Option<String>,

    /// Environment variables for the container.
    pub environment: HashMap<String, String>,

    /// Volume mounts (host:container format).
    pub volumes: Vec<String>,

    /// Port mappings (host:container format).
    pub ports: Vec<String>,

    /// Docker create options (--cpus, --memory, etc.).
    pub options: Option<String>,

    /// Path mappings between host and container paths.
    pub path_mappings: HashMap<String, String>,

    /// Whether this is the job container (vs. a step container).
    pub is_job_container: bool,

    /// Network alias for service containers.
    pub container_network_alias: Option<String>,

    /// User-specified volume mounts from the workflow.
    pub user_mountvolumes: Vec<String>,
}

impl ContainerInfo {
    /// Create a new `ContainerInfo` with just an image name.
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            container_id: None,
            container_name: String::new(),
            network: None,
            entrypoint: None,
            environment: HashMap::new(),
            volumes: Vec::new(),
            ports: Vec::new(),
            options: None,
            path_mappings: HashMap::new(),
            is_job_container: false,
            container_network_alias: None,
            user_mountvolumes: Vec::new(),
        }
    }

    /// Translate a host path to a container path using path mappings.
    pub fn translate_to_container_path(&self, host_path: &str) -> String {
        for (host_prefix, container_prefix) in &self.path_mappings {
            if host_path.starts_with(host_prefix.as_str()) {
                return host_path.replacen(host_prefix.as_str(), container_prefix.as_str(), 1);
            }
        }
        host_path.to_string()
    }

    /// Translate a container path back to a host path.
    pub fn translate_to_host_path(&self, container_path: &str) -> String {
        for (host_prefix, container_prefix) in &self.path_mappings {
            if container_path.starts_with(container_prefix.as_str()) {
                return container_path.replacen(container_prefix.as_str(), host_prefix.as_str(), 1);
            }
        }
        container_path.to_string()
    }

    /// Build the full list of `-v` volume mount arguments for `docker create`.
    pub fn build_volume_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        for vol in &self.volumes {
            args.push("-v".to_string());
            args.push(vol.clone());
        }
        for vol in &self.user_mountvolumes {
            args.push("-v".to_string());
            args.push(vol.clone());
        }
        args
    }

    /// Build the full list of `-p` port mapping arguments for `docker create`.
    pub fn build_port_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        for port in &self.ports {
            args.push("-p".to_string());
            args.push(port.clone());
        }
        args
    }

    /// Build the full list of `-e` environment variable arguments.
    pub fn build_env_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        for (key, value) in &self.environment {
            args.push("-e".to_string());
            args.push(format!("{}={}", key, value));
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_info_new() {
        let container = ContainerInfo::new("ubuntu:22.04");
        assert_eq!(container.image, "ubuntu:22.04");
        assert!(container.container_id.is_none());
    }

    #[test]
    fn test_path_translation() {
        let mut container = ContainerInfo::new("test");
        container.path_mappings.insert(
            "/home/runner/work".to_string(),
            "/github/workspace".to_string(),
        );

        assert_eq!(
            container.translate_to_container_path("/home/runner/work/repo"),
            "/github/workspace/repo"
        );

        assert_eq!(
            container.translate_to_host_path("/github/workspace/repo"),
            "/home/runner/work/repo"
        );
    }

    #[test]
    fn test_build_volume_args() {
        let mut container = ContainerInfo::new("test");
        container.volumes.push("/host:/container".to_string());
        container
            .user_mountvolumes
            .push("/data:/data".to_string());

        let args = container.build_volume_args();
        assert_eq!(args, vec!["-v", "/host:/container", "-v", "/data:/data"]);
    }

    #[test]
    fn test_build_env_args() {
        let mut container = ContainerInfo::new("test");
        container
            .environment
            .insert("FOO".to_string(), "bar".to_string());

        let args = container.build_env_args();
        assert_eq!(args, vec!["-e", "FOO=bar"]);
    }
}
