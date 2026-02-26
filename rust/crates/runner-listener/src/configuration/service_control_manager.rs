// ServiceControlManager mapping `ServiceControlManager.cs`.
// Generates service configuration for systemd (Linux) and launchd (macOS).

use anyhow::{Context, Result};
use runner_common::config_store::RunnerSettings;
use runner_common::constants::{WellKnownConfigFile, WellKnownDirectory};
use runner_common::host_context::HostContext;
use runner_common::tracing::Tracing;
use runner_sdk::TraceWriter;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// Manages service installation for running the runner as a system service.
///
/// Maps `ServiceControlManager` in the C# runner. On Linux this generates
/// a systemd unit file; on macOS a launchd plist.
pub struct ServiceControlManager {
    context: Arc<HostContext>,
    trace: Tracing,
}

impl ServiceControlManager {
    /// Create a new `ServiceControlManager`.
    pub fn new(context: Arc<HostContext>) -> Self {
        let trace = context.get_trace("ServiceControlManager");
        Self { context, trace }
    }

    /// Generate the service configuration files.
    pub fn generate_service_config(&self, settings: &RunnerSettings) -> Result<()> {
        let root_dir = self.context.get_directory(WellKnownDirectory::Root);
        let bin_dir = self.context.get_directory(WellKnownDirectory::Bin);

        #[cfg(target_os = "linux")]
        {
            self.generate_systemd_unit(&root_dir, &bin_dir, settings)?;
        }

        #[cfg(target_os = "macos")]
        {
            self.generate_launchd_plist(&root_dir, &bin_dir, settings)?;
        }

        #[cfg(target_os = "windows")]
        {
            self.generate_windows_service_config(&root_dir, &bin_dir, settings)?;
        }

        // Save service config marker
        let service_path = self.context.get_config_file(WellKnownConfigFile::Service);
        let service_data = serde_json::json!({
            "serviceName": format!("actions.runner.{}", settings.agent_name),
            "serviceDisplayName": format!("GitHub Actions Runner ({})", settings.agent_name),
        });
        std::fs::write(&service_path, serde_json::to_string_pretty(&service_data)?)
            .context("Failed to write service config marker")?;

        self.trace
            .info("Service configuration generated successfully");

        Ok(())
    }

    /// Generate a systemd unit file (Linux).
    #[cfg(target_os = "linux")]
    fn generate_systemd_unit(
        &self,
        root_dir: &PathBuf,
        bin_dir: &PathBuf,
        settings: &RunnerSettings,
    ) -> Result<()> {
        let service_name = format!("actions.runner.{}", settings.agent_name);

        let unit = format!(
            r#"[Unit]
Description=GitHub Actions Runner ({name})
After=network.target

[Service]
ExecStart={bin}/Runner.Listener run --startuptype service
WorkingDirectory={root}
KillMode=process
KillSignal=SIGTERM
TimeoutStopSec=5min
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
"#,
            name = settings.agent_name,
            bin = bin_dir.display(),
            root = root_dir.display(),
        );

        let unit_dir = PathBuf::from("/etc/systemd/system");
        let unit_path = if unit_dir.exists() {
            unit_dir.join(format!("{}.service", service_name))
        } else {
            // Fallback: write to runner root
            root_dir.join(format!("{}.service", service_name))
        };

        self.trace.info(&format!(
            "Writing systemd unit to {:?}",
            unit_path
        ));

        let mut file = std::fs::File::create(&unit_path)
            .context("Failed to create systemd unit file")?;
        file.write_all(unit.as_bytes())?;

        // Also generate install/uninstall scripts
        self.generate_svc_scripts(root_dir, &service_name)?;

        Ok(())
    }

    /// Generate the svc.sh install/uninstall scripts (Linux).
    #[cfg(target_os = "linux")]
    fn generate_svc_scripts(
        &self,
        root_dir: &PathBuf,
        service_name: &str,
    ) -> Result<()> {
        let install_script = format!(
            r#"#!/bin/bash
# Install the runner as a systemd service

SVC_NAME="{name}"

if [ "$(id -u)" -ne 0 ]; then
    echo "Must run as root to install service"
    exit 1
fi

echo "Installing service $SVC_NAME..."
systemctl daemon-reload
systemctl enable "$SVC_NAME"
systemctl start "$SVC_NAME"
echo "Service installed and started."
"#,
            name = service_name,
        );

        let uninstall_script = format!(
            r#"#!/bin/bash
# Uninstall the runner service

SVC_NAME="{name}"

if [ "$(id -u)" -ne 0 ]; then
    echo "Must run as root to uninstall service"
    exit 1
fi

echo "Uninstalling service $SVC_NAME..."
systemctl stop "$SVC_NAME" 2>/dev/null
systemctl disable "$SVC_NAME" 2>/dev/null
rm -f "/etc/systemd/system/$SVC_NAME.service"
systemctl daemon-reload
echo "Service uninstalled."
"#,
            name = service_name,
        );

        let install_path = root_dir.join("svc.sh");
        let mut file = std::fs::File::create(&install_path)?;
        file.write_all(install_script.as_bytes())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&install_path, std::fs::Permissions::from_mode(0o755))?;
        }

        let uninstall_path = root_dir.join("svc-uninstall.sh");
        let mut file = std::fs::File::create(&uninstall_path)?;
        file.write_all(uninstall_script.as_bytes())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&uninstall_path, std::fs::Permissions::from_mode(0o755))?;
        }

        Ok(())
    }

    /// Generate a launchd plist (macOS).
    #[cfg(target_os = "macos")]
    fn generate_launchd_plist(
        &self,
        root_dir: &PathBuf,
        bin_dir: &PathBuf,
        settings: &RunnerSettings,
    ) -> Result<()> {
        let label = format!("actions.runner.{}", settings.agent_name);

        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}/Runner.Listener</string>
        <string>run</string>
        <string>--startuptype</string>
        <string>service</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{root}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{root}/_diag/runner.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>{root}/_diag/runner.stderr.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>SessionCreate</key>
    <true/>
</dict>
</plist>
"#,
            label = label,
            bin = bin_dir.display(),
            root = root_dir.display(),
        );

        let plist_path = root_dir.join(format!("{}.plist", label));

        self.trace.info(&format!(
            "Writing launchd plist to {:?}",
            plist_path
        ));

        std::fs::write(&plist_path, &plist)
            .context("Failed to write launchd plist")?;

        // Generate svc.sh for macOS
        let svc_script = format!(
            r#"#!/bin/bash
# Service management script for macOS

LABEL="{label}"
PLIST="{root}/{label}.plist"
ACTION="${{1:-status}}"

case "$ACTION" in
    install)
        cp "$PLIST" ~/Library/LaunchAgents/
        launchctl load ~/Library/LaunchAgents/"$LABEL".plist
        echo "Service installed and loaded."
        ;;
    uninstall)
        launchctl unload ~/Library/LaunchAgents/"$LABEL".plist 2>/dev/null
        rm -f ~/Library/LaunchAgents/"$LABEL".plist
        echo "Service uninstalled."
        ;;
    start)
        launchctl start "$LABEL"
        echo "Service started."
        ;;
    stop)
        launchctl stop "$LABEL"
        echo "Service stopped."
        ;;
    status)
        launchctl list | grep "$LABEL" && echo "Running" || echo "Not running"
        ;;
    *)
        echo "Usage: $0 {{install|uninstall|start|stop|status}}"
        ;;
esac
"#,
            label = label,
            root = root_dir.display(),
        );

        let svc_path = root_dir.join("svc.sh");
        let mut file = std::fs::File::create(&svc_path)?;
        file.write_all(svc_script.as_bytes())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&svc_path, std::fs::Permissions::from_mode(0o755))?;
        }

        Ok(())
    }

    /// Generate Windows service configuration.
    #[cfg(target_os = "windows")]
    fn generate_windows_service_config(
        &self,
        root_dir: &PathBuf,
        bin_dir: &PathBuf,
        settings: &RunnerSettings,
    ) -> Result<()> {
        let service_name = format!("actions.runner.{}", settings.agent_name);

        let install_script = format!(
            r#"@echo off
REM Install the runner as a Windows service
sc.exe create "{name}" binPath= "{bin}\Runner.Listener.exe run --startuptype service" start= auto
sc.exe description "{name}" "GitHub Actions Runner ({display})"
sc.exe start "{name}"
echo Service installed and started.
"#,
            name = service_name,
            bin = bin_dir.display(),
            display = settings.agent_name,
        );

        let install_path = root_dir.join("install-svc.cmd");
        std::fs::write(&install_path, &install_script)?;

        let uninstall_script = format!(
            r#"@echo off
REM Uninstall the runner service
sc.exe stop "{name}" 2>nul
sc.exe delete "{name}"
echo Service uninstalled.
"#,
            name = service_name,
        );

        let uninstall_path = root_dir.join("uninstall-svc.cmd");
        std::fs::write(&uninstall_path, &uninstall_script)?;

        self.trace.info("Windows service scripts generated");

        Ok(())
    }
}
