use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use walkdir::WalkDir;

use crate::exec::{ExecOpts, ExecResult};

/// Fixed guest mountpoint for Moltis instance data inside sandbox containers.
pub const SANDBOX_GUEST_DATA_DIR: &str = "/moltis/data";
/// Fixed guest workdir for sandboxed exec/process.
pub const SANDBOX_GUEST_WORKDIR: &str = "/moltis/workdir";
/// Fixed guest temp dir for sandboxed exec/process.
pub const SANDBOX_GUEST_TMPDIR: &str = "/moltis/workdir/tmp";

const SANDBOX_CONTAINER_CONTRACT_VERSION: &str = "sandbox_contract_v1";
const SANDBOX_LABEL_MANAGED: &str = "moltis.managed";
const SANDBOX_LABEL_ROLE: &str = "moltis.role";
const SANDBOX_LABEL_INSTANCE_ID: &str = "moltis.instance_id";
const SANDBOX_LABEL_IMAGE_REF: &str = "moltis.image_ref";
const SANDBOX_LABEL_CONTRACT_VERSION: &str = "moltis.contract_version";
const SANDBOX_LABEL_WORKDIR: &str = "moltis.workdir";
const SANDBOX_LABEL_TMPDIR: &str = "moltis.tmpdir";

fn sandbox_instance_id() -> String {
    // Stable per-instance identifier: hash the canonicalized data_dir.
    // - Must not be user-readable (paths vary by environment).
    // - Must be stable across restarts with the same data_dir.
    let data_dir = normalize_existing_mount_contract_path(&moltis_config::data_dir());
    let mut h = Sha256::new();
    h.update(b"moltis_sandbox_instance_id_v1\0");
    h.update(data_dir.display().to_string().as_bytes());
    let digest = h.finalize();
    digest[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

fn public_data_view_dir(base_data_dir: &str, sandbox_key: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(base_data_dir)
        .join(".sandbox_views")
        .join(sandbox_key)
}

fn copy_file_or_empty(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    match std::fs::read_to_string(src) {
        Ok(content) => std::fs::write(dst, content)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::write(dst, "")?,
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn remove_public_entry(path: &std::path::Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path)?,
        Ok(_) => std::fs::remove_file(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn copy_public_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    if !src.exists() {
        return Ok(());
    }

    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .context("walkdir entry escaped source root")?;
        let target = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }

        if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
            continue;
        }

        if entry.file_type().is_symlink() {
            warn!(
                path = %entry.path().display(),
                reason_code = "sandbox_public_data_symlink_skipped",
                "skipping symlink while preparing sandbox public data view"
            );
        }
    }

    Ok(())
}

fn prepare_public_data_view(
    base_data_dir: &str,
    sandbox_key: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let view_dir = public_data_view_dir(base_data_dir, sandbox_key);
    std::fs::create_dir_all(&view_dir)?;

    // Only expose public workspace files and discoverable skill definitions to
    // sandboxed exec.
    let base = std::path::PathBuf::from(base_data_dir);
    copy_file_or_empty(&base.join("USER.md"), &view_dir.join("USER.md"))?;
    copy_file_or_empty(&base.join("PEOPLE.md"), &view_dir.join("PEOPLE.md"))?;
    remove_public_entry(&view_dir.join("skills"))?;
    remove_public_entry(&view_dir.join(".moltis/skills"))?;
    copy_public_dir_recursive(&base.join("skills"), &view_dir.join("skills"))?;
    copy_public_dir_recursive(
        &base.join(".moltis/skills"),
        &view_dir.join(".moltis/skills"),
    )?;

    Ok(view_dir)
}

fn normalize_existing_mount_contract_path(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Sandbox mode controlling when sandboxing is applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SandboxMode {
    Off,
    NonMain,
    #[default]
    All,
}

impl std::fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => f.write_str("off"),
            Self::NonMain => f.write_str("non-main"),
            Self::All => f.write_str("all"),
        }
    }
}

/// Scope key determines sandbox container reuse boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxScopeKey {
    SessionId,
    SessionKey,
}

impl Default for SandboxScopeKey {
    fn default() -> Self {
        Self::SessionId
    }
}

impl std::fmt::Display for SandboxScopeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionId => f.write_str("session_id"),
            Self::SessionKey => f.write_str("session_key"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupContainerPolicy {
    Reset,
    Reuse,
}

impl Default for StartupContainerPolicy {
    fn default() -> Self {
        Self::Reset
    }
}

/// Mount mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum WorkspaceMount {
    None,
    #[default]
    Ro,
    Rw,
}

impl std::fmt::Display for WorkspaceMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Ro => f.write_str("ro"),
            Self::Rw => f.write_str("rw"),
        }
    }
}

/// Backing type for the sandbox data mount (`/moltis/data`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataMountType {
    Bind,
    Volume,
}

impl std::fmt::Display for DataMountType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind => f.write_str("bind"),
            Self::Volume => f.write_str("volume"),
        }
    }
}

/// Resource limits for sandboxed execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceLimits {
    /// Memory limit (e.g. "512M", "1G").
    pub memory_limit: Option<String>,
    /// CPU quota as a fraction (e.g. 0.5 = half a core, 2.0 = two cores).
    pub cpu_quota: Option<f64>,
    /// Maximum number of PIDs.
    pub pids_max: Option<u32>,
}

/// Configuration for sandbox behavior (docker-only one-cut).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    pub scope_key: SandboxScopeKey,
    pub idle_ttl_secs: u64,
    pub data_mount: WorkspaceMount,
    pub data_mount_type: Option<DataMountType>,
    pub data_mount_source: Option<String>,
    #[serde(default)]
    pub mounts: Vec<SandboxMount>,
    #[serde(default)]
    pub mount_allowlist: Vec<std::path::PathBuf>,
    pub image: Option<String>,
    pub no_network: bool,
    pub startup_container_policy: StartupContainerPolicy,
    pub resource_limits: ResourceLimits,
    /// IANA timezone injected as `TZ` env var.
    pub timezone: Option<String>,
}

/// External mount configuration entry for sandbox containers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxMount {
    pub host_dir: std::path::PathBuf,
    pub guest_dir: std::path::PathBuf,
    pub mode: WorkspaceMount,
}

impl Default for SandboxMount {
    fn default() -> Self {
        Self {
            host_dir: std::path::PathBuf::new(),
            guest_dir: std::path::PathBuf::new(),
            mode: WorkspaceMount::Ro,
        }
    }
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::All,
            scope_key: SandboxScopeKey::SessionId,
            idle_ttl_secs: 0,
            data_mount: WorkspaceMount::Ro,
            data_mount_type: None,
            data_mount_source: None,
            mounts: Vec::new(),
            mount_allowlist: Vec::new(),
            image: None,
            no_network: true,
            startup_container_policy: StartupContainerPolicy::Reset,
            resource_limits: ResourceLimits::default(),
            timezone: None,
        }
    }
}

impl TryFrom<&moltis_config::schema::SandboxConfig> for SandboxConfig {
    type Error = anyhow::Error;

    fn try_from(cfg: &moltis_config::schema::SandboxConfig) -> Result<Self> {
        if let Some(ref scope) = cfg.scope {
            anyhow::bail!(
                "SANDBOX_LEGACY_SCOPE_REMOVED: tools.exec.sandbox.scope is no longer supported (got {scope}); remove it"
            );
        }
        if let Some(ref backend) = cfg.backend {
            anyhow::bail!(
                "SANDBOX_LEGACY_BACKEND_REMOVED: tools.exec.sandbox.backend is no longer supported (got {backend}); remove it"
            );
        }
        if let Some(packages) = cfg.packages.as_ref() {
            anyhow::bail!(
                "SANDBOX_LEGACY_BUILD_PATH_REMOVED: tools.exec.sandbox.packages is no longer supported ({} entries); remove it",
                packages.len()
            );
        }
        if let Some(ref prefix) = cfg.container_prefix {
            anyhow::bail!(
                "SANDBOX_LEGACY_CONTAINER_PREFIX_REMOVED: tools.exec.sandbox.container_prefix is no longer supported (got {prefix}); remove it"
            );
        }

        let mode = match cfg.mode.trim().to_ascii_lowercase().as_str() {
            "off" => SandboxMode::Off,
            "all" => SandboxMode::All,
            "non-main" => SandboxMode::NonMain,
            other => anyhow::bail!("SANDBOX_CONFIG_INVALID: unknown sandbox mode: {other}"),
        };

        let scope_key = match cfg.scope_key.trim() {
            "session_id" => SandboxScopeKey::SessionId,
            "session_key" => SandboxScopeKey::SessionKey,
            other => anyhow::bail!("SANDBOX_CONFIG_INVALID: unknown scope_key: {other}"),
        };

        let data_mount = match cfg.data_mount.trim() {
            "none" => WorkspaceMount::None,
            "ro" => WorkspaceMount::Ro,
            "rw" => WorkspaceMount::Rw,
            other => anyhow::bail!("SANDBOX_CONFIG_INVALID: unknown data_mount: {other}"),
        };

        let data_mount_type = match cfg.data_mount_type.as_deref().map(str::trim) {
            None => None,
            Some("") => None,
            Some("bind") => Some(DataMountType::Bind),
            Some("volume") => Some(DataMountType::Volume),
            Some(other) => anyhow::bail!("SANDBOX_CONFIG_INVALID: unknown data_mount_type: {other}"),
        };

        let mounts = cfg
            .mounts
            .iter()
            .map(|m| {
                let mode = match m.mode.trim() {
                    "ro" => WorkspaceMount::Ro,
                    "rw" => WorkspaceMount::Rw,
                    "none" => WorkspaceMount::None,
                    other => anyhow::bail!("SANDBOX_CONFIG_INVALID: unknown mounts[].mode: {other}"),
                };
                Ok(SandboxMount {
                    host_dir: std::path::PathBuf::from(m.host_dir.trim()),
                    guest_dir: std::path::PathBuf::from(m.guest_dir.trim()),
                    mode,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mount_allowlist = cfg
            .mount_allowlist
            .iter()
            .map(|p| std::path::PathBuf::from(p.trim()))
            .collect::<Vec<_>>();

        let startup_container_policy = match cfg.startup_container_policy.trim() {
            "reset" => StartupContainerPolicy::Reset,
            "reuse" => StartupContainerPolicy::Reuse,
            other => anyhow::bail!(
                "SANDBOX_CONFIG_INVALID: unknown startup_container_policy: {other} (expected reset|reuse)"
            ),
        };

        if mode != SandboxMode::Off
            && cfg
                .image
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .is_none()
        {
            anyhow::bail!(
                "SANDBOX_IMAGE_MISSING: tools.exec.sandbox.image is required when sandbox is enabled"
            );
        }

        Ok(Self {
            mode,
            scope_key,
            idle_ttl_secs: cfg.idle_ttl_secs,
            data_mount,
            data_mount_type,
            data_mount_source: cfg.data_mount_source.clone(),
            mounts,
            mount_allowlist,
            image: cfg.image.clone(),
            no_network: cfg.no_network,
            startup_container_policy,
            resource_limits: ResourceLimits {
                memory_limit: cfg.resource_limits.memory_limit.clone(),
                cpu_quota: cfg.resource_limits.cpu_quota,
                pids_max: cfg.resource_limits.pids_max,
            },
            timezone: None, // set by gateway from user profile
        })
    }
}

/// Sandbox identifier — session or agent scoped.
#[derive(Debug, Clone)]
pub struct SandboxId {
    pub scope_key: SandboxScopeKey,
    pub key: String,
}

impl std::fmt::Display for SandboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}/{}", self.scope_key, self.key)
    }
}

/// Trait for sandbox implementations.
#[async_trait]
pub trait Sandbox: Send + Sync {
    fn backend_name(&self) -> &'static str;
    async fn ensure_ready(&self, id: &SandboxId) -> Result<()>;
    async fn exec(&self, id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult>;
    async fn cleanup(&self, id: &SandboxId) -> Result<()>;
}

async fn ensure_docker_daemon_available() -> Result<()> {
    let output = tokio::process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            anyhow::bail!(
                "SANDBOX_BACKEND_UNAVAILABLE: docker daemon is not accessible: {}",
                stderr.trim()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("SANDBOX_BACKEND_UNAVAILABLE: docker CLI not found on PATH");
        }
        Err(e) => anyhow::bail!("SANDBOX_BACKEND_UNAVAILABLE: docker info failed: {e}"),
    }
}

/// Docker-based sandbox implementation (Moltis does not build/pull images).
pub struct DockerSandbox {
    config: SandboxConfig,
    instance_id: String,
}

impl DockerSandbox {
    pub fn new(config: SandboxConfig) -> Self {
        Self {
            config,
            instance_id: sandbox_instance_id(),
        }
    }

    fn image_ref(&self) -> Result<&str> {
        self.config
            .image
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "SANDBOX_IMAGE_MISSING: tools.exec.sandbox.image is required when sandbox is enabled"
                )
            })
    }

    fn normalize_bind_mount_source_for_compare(source: &str) -> String {
        if !source.starts_with('/') {
            return source.trim().to_string();
        }

        let mut stack: Vec<&str> = Vec::new();
        for segment in source.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    let _ = stack.pop();
                }
                other => stack.push(other),
            }
        }

        if stack.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", stack.join("/"))
        }
    }

    fn container_name(&self, id: &SandboxId) -> String {
        id.key.clone()
    }

    fn resource_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let limits = &self.config.resource_limits;
        if let Some(ref mem) = limits.memory_limit {
            args.extend(["--memory".to_string(), mem.clone()]);
        }
        if let Some(cpu) = limits.cpu_quota {
            args.extend(["--cpus".to_string(), cpu.to_string()]);
        }
        if let Some(pids) = limits.pids_max {
            args.extend(["--pids-limit".to_string(), pids.to_string()]);
        }
        args
    }

    fn data_mount_args(&self, id: &SandboxId) -> Result<Vec<String>> {
        let mode = match self.config.data_mount {
            WorkspaceMount::Ro => "ro",
            WorkspaceMount::Rw => "rw",
            WorkspaceMount::None => {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox enabled requires tools.exec.sandbox.data_mount=ro|rw (none is not allowed)"
                )
            }
        };

        let mount_type = self.config.data_mount_type.ok_or_else(|| {
            anyhow::anyhow!(
                "SANDBOX_CONFIG_INVALID: tools.exec.sandbox.data_mount_type is required when sandbox is enabled"
            )
        })?;

        let mount_source = self
            .config
            .data_mount_source
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "SANDBOX_CONFIG_INVALID: tools.exec.sandbox.data_mount_source is required when sandbox is enabled"
                )
            })?;

        let mount_source_to_use = match mount_type {
            DataMountType::Bind => {
                if !std::path::Path::new(mount_source).is_absolute() {
                    anyhow::bail!(
                        "SANDBOX_CONFIG_INVALID: tools.exec.sandbox.data_mount_source must be an absolute path when tools.exec.sandbox.data_mount_type=bind"
                    );
                }
                if mount_source.contains(':') {
                    anyhow::bail!(
                        "SANDBOX_CONFIG_INVALID: tools.exec.sandbox.data_mount_source must not contain ':' when tools.exec.sandbox.data_mount_type=bind"
                    );
                }
                let configured_source = std::path::PathBuf::from(mount_source);
                let effective_data_dir = moltis_config::data_dir();
                if normalize_existing_mount_contract_path(&configured_source)
                    != normalize_existing_mount_contract_path(&effective_data_dir)
                {
                    warn!(
                        event = "sandbox_data_mount_rejected",
                        reason_code = "sandbox_bind_source_must_equal_data_dir",
                        decision = "reject",
                        policy = "sandbox_data_mount_contract",
                        configured_source = %configured_source.display(),
                        effective_data_dir = %effective_data_dir.display(),
                        "rejecting sandbox bind source outside effective data_dir contract"
                    );
                    anyhow::bail!(
                        "SANDBOX_CONFIG_INVALID: tools.exec.sandbox.data_mount_source must resolve to the effective Moltis data_dir; set tools.exec.sandbox.data_mount_source=\"{}\"",
                        effective_data_dir.display()
                    );
                }
                let view_dir = prepare_public_data_view(mount_source, &id.key)?;
                view_dir.display().to_string()
            }
            DataMountType::Volume => {
                if mount_source.contains('/')
                    || mount_source.contains('\\')
                    || mount_source.contains(':')
                    || mount_source.chars().any(char::is_whitespace)
                {
                    anyhow::bail!(
                        "SANDBOX_CONFIG_INVALID: tools.exec.sandbox.data_mount_source must be a Docker volume name when tools.exec.sandbox.data_mount_type=volume"
                    );
                }
                mount_source.to_string()
            }
        };

        Ok(vec![
            "-v".to_string(),
            format!("{mount_source_to_use}:{SANDBOX_GUEST_DATA_DIR}:{mode}"),
        ])
    }

    fn external_mount_args(&self) -> Result<Vec<String>> {
        const GUEST_PREFIX: &str = "/mnt/host/";

        if self.config.mounts.is_empty() {
            return Ok(Vec::new());
        }
        if self.config.mount_allowlist.is_empty() {
            anyhow::bail!(
                "SANDBOX_CONFIG_INVALID: sandbox mounts are configured but mount_allowlist is empty (deny-by-default)"
            );
        }

        let mut allow_roots = Vec::with_capacity(self.config.mount_allowlist.len());
        for root in &self.config.mount_allowlist {
            if !root.is_absolute() {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mount_allowlist entry must be an absolute path: {}",
                    root.display()
                );
            }
            let canonical = std::fs::canonicalize(root).with_context(|| {
                format!(
                    "canonicalize sandbox mount_allowlist entry: {}",
                    root.display()
                )
            })?;
            if !canonical.is_dir() {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mount_allowlist entry must be a directory: {}",
                    canonical.display()
                );
            }
            allow_roots.push(canonical);
        }

        let mut args = Vec::new();
        for (i, mount) in self.config.mounts.iter().enumerate() {
            if mount.host_dir.as_os_str().is_empty() {
                anyhow::bail!("SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].host_dir is empty");
            }
            if !mount.host_dir.is_absolute() {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].host_dir must be an absolute path: {}",
                    mount.host_dir.display()
                );
            }
            let canonical_host = std::fs::canonicalize(&mount.host_dir).with_context(|| {
                format!(
                    "canonicalize sandbox mounts[{i}].host_dir: {}",
                    mount.host_dir.display()
                )
            })?;
            if !canonical_host.is_dir() {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].host_dir must be a directory: {}",
                    canonical_host.display()
                );
            }
            if !allow_roots
                .iter()
                .any(|root| canonical_host.starts_with(root))
            {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].host_dir is outside mount_allowlist: {}",
                    canonical_host.display()
                );
            }

            if mount.guest_dir.as_os_str().is_empty() {
                anyhow::bail!("SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].guest_dir is empty");
            }
            if !mount.guest_dir.is_absolute() {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].guest_dir must be an absolute path: {}",
                    mount.guest_dir.display()
                );
            }
            let guest_str = mount.guest_dir.display().to_string();
            if !guest_str.starts_with(GUEST_PREFIX) {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].guest_dir must be under {GUEST_PREFIX} (got: {guest_str})"
                );
            }
            if guest_str == "/"
                || guest_str == "/proc"
                || guest_str == "/sys"
                || guest_str == "/dev"
            {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].guest_dir is a protected path: {guest_str}"
                );
            }
            if mount.guest_dir.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }) {
                anyhow::bail!(
                    "SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].guest_dir must not contain '.' or '..': {guest_str}"
                );
            }

            let mode = match mount.mode {
                WorkspaceMount::Ro => "ro",
                WorkspaceMount::Rw => "rw",
                WorkspaceMount::None => {
                    anyhow::bail!("SANDBOX_CONFIG_INVALID: sandbox mounts[{i}].mode must be \"ro\" or \"rw\"")
                }
            };

            args.push("-v".to_string());
            args.push(format!(
                "{}:{}:{}",
                canonical_host.display(),
                mount.guest_dir.display(),
                mode
            ));
        }
        Ok(args)
    }

    fn docker_run_args(&self, name: &str, image: &str, id: &SandboxId) -> Result<Vec<String>> {
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            name.to_string(),
        ];

        args.extend([
            "--label".to_string(),
            format!("{SANDBOX_LABEL_MANAGED}=true"),
            "--label".to_string(),
            format!("{SANDBOX_LABEL_ROLE}=sandbox"),
            "--label".to_string(),
            format!("{SANDBOX_LABEL_INSTANCE_ID}={}", self.instance_id),
            "--label".to_string(),
            format!("{SANDBOX_LABEL_IMAGE_REF}={image}"),
            "--label".to_string(),
            format!("{SANDBOX_LABEL_CONTRACT_VERSION}={SANDBOX_CONTAINER_CONTRACT_VERSION}"),
            "--label".to_string(),
            format!("{SANDBOX_LABEL_WORKDIR}={SANDBOX_GUEST_WORKDIR}"),
            "--label".to_string(),
            format!("{SANDBOX_LABEL_TMPDIR}={SANDBOX_GUEST_TMPDIR}"),
        ]);

        if self.config.no_network {
            args.push("--network=none".to_string());
        }

        if let Some(ref tz) = self.config.timezone {
            args.extend(["-e".to_string(), format!("TZ={tz}")]);
        }

        args.extend(["-w".to_string(), SANDBOX_GUEST_WORKDIR.to_string()]);
        args.extend([
            "-e".to_string(),
            format!("MOLTIS_DATA_DIR={SANDBOX_GUEST_DATA_DIR}"),
            "-e".to_string(),
            format!("HOME={SANDBOX_GUEST_WORKDIR}"),
            "-e".to_string(),
            format!("TMPDIR={SANDBOX_GUEST_TMPDIR}"),
        ]);

        args.extend(self.resource_args());
        args.extend(self.data_mount_args(id)?);
        args.extend(self.external_mount_args()?);

        args.push(image.to_string());
        args.extend([
            "sh".to_string(),
            "-lc".to_string(),
            format!("mkdir -p {SANDBOX_GUEST_TMPDIR} && exec sleep infinity"),
        ]);
        Ok(args)
    }

    async fn container_contract_matches(&self, name: &str, id: &SandboxId) -> Result<bool> {
        let inspect = tokio::process::Command::new("docker")
            .args(["inspect", name])
            .output()
            .await?;
        if !inspect.status.success() {
            return Ok(false);
        }
        let json: serde_json::Value = serde_json::from_slice(&inspect.stdout)?;
        let entry = json.as_array().and_then(|a| a.first());
        let Some(entry) = entry else {
            return Ok(false);
        };

        let labels = entry
            .pointer("/Config/Labels")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let expected_image = self.image_ref()?;
        let label_ok = labels
            .get(SANDBOX_LABEL_CONTRACT_VERSION)
            .and_then(|v| v.as_str())
            == Some(SANDBOX_CONTAINER_CONTRACT_VERSION)
            && labels
                .get(SANDBOX_LABEL_MANAGED)
                .and_then(|v| v.as_str())
                == Some("true")
            && labels.get(SANDBOX_LABEL_ROLE).and_then(|v| v.as_str()) == Some("sandbox")
            && labels
                .get(SANDBOX_LABEL_INSTANCE_ID)
                .and_then(|v| v.as_str())
                == Some(self.instance_id.as_str())
            && labels
                .get(SANDBOX_LABEL_IMAGE_REF)
                .and_then(|v| v.as_str())
                == Some(expected_image)
            && labels.get(SANDBOX_LABEL_WORKDIR).and_then(|v| v.as_str())
                == Some(SANDBOX_GUEST_WORKDIR)
            && labels.get(SANDBOX_LABEL_TMPDIR).and_then(|v| v.as_str()) == Some(SANDBOX_GUEST_TMPDIR);

        if !label_ok {
            return Ok(false);
        }

        let workdir_ok = entry
            .pointer("/Config/WorkingDir")
            .and_then(|v| v.as_str())
            == Some(SANDBOX_GUEST_WORKDIR);
        if !workdir_ok {
            return Ok(false);
        }

        let envs = entry
            .pointer("/Config/Env")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let env_ok = envs.iter().filter_map(|v| v.as_str()).any(|v| {
            v == format!("MOLTIS_DATA_DIR={SANDBOX_GUEST_DATA_DIR}")
        }) && envs.iter().filter_map(|v| v.as_str()).any(|v| v == format!("HOME={SANDBOX_GUEST_WORKDIR}"))
            && envs
                .iter()
                .filter_map(|v| v.as_str())
                .any(|v| v == format!("TMPDIR={SANDBOX_GUEST_TMPDIR}"));
        if !env_ok {
            return Ok(false);
        }

        if self.config.no_network {
            let network_mode = entry
                .pointer("/HostConfig/NetworkMode")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if network_mode != "none" {
                return Ok(false);
            }
        }

        let mounts = entry
            .pointer("/Mounts")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let expected_data_mount_type = self.config.data_mount_type.ok_or_else(|| {
            anyhow::anyhow!("SANDBOX_CONFIG_INVALID: data_mount_type missing")
        })?;
        let expected_mount_source = self
            .config
            .data_mount_source
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("SANDBOX_CONFIG_INVALID: data_mount_source missing"))?;

        let expected_rw = matches!(self.config.data_mount, WorkspaceMount::Rw);
        let expected_data_source_normalized = match expected_data_mount_type {
            DataMountType::Bind => {
                let view_dir = public_data_view_dir(expected_mount_source, &id.key)
                    .display()
                    .to_string();
                Self::normalize_bind_mount_source_for_compare(&view_dir)
            }
            DataMountType::Volume => expected_mount_source.to_string(),
        };

        let mut data_mount_ok = false;
        for m in &mounts {
            let dest = m.pointer("/Destination").and_then(|v| v.as_str()).unwrap_or("");
            if dest != SANDBOX_GUEST_DATA_DIR {
                continue;
            }
            let mount_type = m.pointer("/Type").and_then(|v| v.as_str()).unwrap_or("");
            let rw = m.pointer("/RW").and_then(|v| v.as_bool()).unwrap_or(false);
            if rw != expected_rw {
                continue;
            }

            match expected_data_mount_type {
                DataMountType::Bind => {
                    let source = m.pointer("/Source").and_then(|v| v.as_str()).unwrap_or("");
                    let source_norm = Self::normalize_bind_mount_source_for_compare(source);
                    if mount_type == "bind" && source_norm == expected_data_source_normalized {
                        data_mount_ok = true;
                    }
                }
                DataMountType::Volume => {
                    let name = m.pointer("/Name").and_then(|v| v.as_str()).unwrap_or("");
                    if mount_type == "volume" && name == expected_data_source_normalized {
                        data_mount_ok = true;
                    }
                }
            }
        }
        if !data_mount_ok {
            return Ok(false);
        }

        // External mounts: ensure every configured mount exists with correct guest_dir and RW.
        for mount in &self.config.mounts {
            let expected_dest = mount.guest_dir.display().to_string();
            let expected_rw = matches!(mount.mode, WorkspaceMount::Rw);

            let mut found = false;
            for m in &mounts {
                let dest = m.pointer("/Destination").and_then(|v| v.as_str()).unwrap_or("");
                if dest != expected_dest {
                    continue;
                }
                let mount_type = m.pointer("/Type").and_then(|v| v.as_str()).unwrap_or("");
                if mount_type != "bind" {
                    continue;
                }
                let rw = m.pointer("/RW").and_then(|v| v.as_bool()).unwrap_or(false);
                if rw != expected_rw {
                    continue;
                }

                let source = m.pointer("/Source").and_then(|v| v.as_str()).unwrap_or("");
                let source_norm = Self::normalize_bind_mount_source_for_compare(source);
                let expected_source_norm = Self::normalize_bind_mount_source_for_compare(
                    &normalize_existing_mount_contract_path(&mount.host_dir)
                        .display()
                        .to_string(),
                );
                if source_norm == expected_source_norm {
                    found = true;
                    break;
                }
            }
            if !found {
                return Ok(false);
            }
        }

        Ok(true)
    }

    async fn list_managed_container_names(&self) -> Result<Vec<String>> {
        let output = tokio::process::Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                "label=moltis.managed=true",
                "--filter",
                "label=moltis.role=sandbox",
                "--filter",
                &format!("{SANDBOX_LABEL_INSTANCE_ID}={}", self.instance_id),
                "--format",
                "{{.Names}}",
            ])
            .output()
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "SANDBOX_CONTAINER_CLEANUP_FAILED: docker ps failed while listing managed containers: {}",
                stderr.trim()
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect())
    }

    async fn remove_container_by_name(&self, name: &str, reason_code: &'static str) -> Result<()> {
        let output = tokio::process::Command::new("docker")
            .args(["rm", "-f", name])
            .output()
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "SANDBOX_CONTAINER_DELETE_FAILED: docker rm -f failed for {name} (reason_code={reason_code}): {}",
                stderr.trim()
            );
        }
        Ok(())
    }

    pub async fn startup_ensure_ready(&self) -> Result<()> {
        ensure_docker_daemon_available().await?;

        let image = self.image_ref()?;
        let img = tokio::process::Command::new("docker")
            .args(["image", "inspect", image])
            .output()
            .await?;
        if !img.status.success() {
            let stderr = String::from_utf8_lossy(&img.stderr);
            anyhow::bail!(
                "SANDBOX_IMAGE_MISSING: local docker image not found: {image}: {}",
                stderr.trim()
            );
        }

        // Minimal image contract validation: sh must exist and workdir/env must be usable.
        let mut validate_args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "-w".to_string(),
            SANDBOX_GUEST_WORKDIR.to_string(),
            "-e".to_string(),
            format!("HOME={SANDBOX_GUEST_WORKDIR}"),
            "-e".to_string(),
            format!("TMPDIR={SANDBOX_GUEST_TMPDIR}"),
        ];
        if self.config.no_network {
            validate_args.push("--network=none".to_string());
        }
        validate_args.push(image.to_string());
        validate_args.extend([
            "sh".to_string(),
            "-lc".to_string(),
            format!(
                "mkdir -p {SANDBOX_GUEST_TMPDIR} && test \"$(pwd)\" = \"{SANDBOX_GUEST_WORKDIR}\""
            ),
        ]);

        let out = tokio::process::Command::new("docker")
            .args(&validate_args)
            .output()
            .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "SANDBOX_IMAGE_CONTRACT_INVALID: runtime image failed contract validation (image={image}): {}",
                stderr.trim()
            );
        }

        // Only after config/image checks pass, apply startup container policy.
        let names = self.list_managed_container_names().await?;
        if names.is_empty() {
            return Ok(());
        }

        match self.config.startup_container_policy {
            StartupContainerPolicy::Reset => {
                for name in names {
                    warn!(
                        event = "sandbox_startup_container_cleanup",
                        reason_code = "sandbox_startup_policy_reset",
                        decision = "delete",
                        policy = "startup_container_policy",
                        container_name = %name,
                        "deleting managed sandbox container on startup (reset)"
                    );
                    self.remove_container_by_name(&name, "sandbox_startup_policy_reset")
                        .await
                        .map_err(|e| anyhow::anyhow!("SANDBOX_CONTAINER_CLEANUP_FAILED: {e}"))?;
                }
            }
            StartupContainerPolicy::Reuse => {
                for name in names {
                    let running = tokio::process::Command::new("docker")
                        .args(["inspect", "--format", "{{.State.Running}}", &name])
                        .output()
                        .await?;
                    let is_running = running.status.success()
                        && String::from_utf8_lossy(&running.stdout).trim() == "true";

                    let id = SandboxId {
                        scope_key: self.config.scope_key.clone(),
                        key: name.clone(),
                    };
                    let contract_ok = is_running && self.container_contract_matches(&name, &id).await?;
                    if contract_ok {
                        info!(
                            event = "sandbox_startup_container_reuse",
                            reason_code = "sandbox_startup_policy_reuse",
                            decision = "keep",
                            policy = "startup_container_policy",
                            container_name = %name,
                            "reusing managed sandbox container on startup (reuse)"
                        );
                        continue;
                    }

                    warn!(
                        event = "sandbox_startup_container_cleanup",
                        reason_code = "sandbox_startup_policy_reuse_delete",
                        decision = "delete",
                        policy = "startup_container_policy",
                        container_name = %name,
                        is_running,
                        "deleting managed sandbox container on startup (reuse)"
                    );
                    self.remove_container_by_name(&name, "sandbox_startup_policy_reuse_delete")
                        .await
                        .map_err(|e| anyhow::anyhow!("SANDBOX_CONTAINER_CLEANUP_FAILED: {e}"))?;
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Sandbox for DockerSandbox {
    fn backend_name(&self) -> &'static str {
        "docker"
    }

    async fn ensure_ready(&self, id: &SandboxId) -> Result<()> {
        ensure_docker_daemon_available().await?;

        let name = self.container_name(id);
        let image = self.image_ref()?;

        // One-cut: Moltis never builds/pulls images. The runtime image must exist locally.
        let img_ok = tokio::process::Command::new("docker")
            .args(["image", "inspect", image])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success());
        if !img_ok {
            anyhow::bail!(
                "SANDBOX_IMAGE_MISSING: local docker image not found: {image}. Remediation: run `docker image inspect {image}`."
            );
        }

        let check = tokio::process::Command::new("docker")
            .args(["inspect", "--format", "{{.State.Running}}", &name])
            .output()
            .await;

        if let Ok(output) = check
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim() == "true" {
                if self.container_contract_matches(&name, id).await? {
                    return Ok(());
                }

                warn!(
                    event = "sandbox_container_rebuild",
                    reason_code = "sandbox_container_contract_mismatch",
                    decision = "rebuild",
                    policy = "sandbox_container_contract",
                    container_name = %name,
                    "sandbox container contract mismatch, removing and recreating"
                );
                self.remove_container_by_name(&name, "sandbox_container_contract_mismatch")
                    .await?;
            } else {
                warn!(
                    event = "sandbox_container_rebuild",
                    reason_code = "sandbox_container_not_running",
                    decision = "rebuild",
                    policy = "sandbox_container_lifecycle",
                    container_name = %name,
                    running = %stdout.trim(),
                    "sandbox container not running, removing and recreating"
                );
                self.remove_container_by_name(&name, "sandbox_container_not_running")
                    .await?;
            }
        }

        let args = self.docker_run_args(&name, image, id)?;
        let output = tokio::process::Command::new("docker")
            .args(&args)
            .output()
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "SANDBOX_CONTAINER_CREATE_FAILED: docker run failed for {name} (image={image}): {}",
                stderr.trim()
            );
        }

        if !self.container_contract_matches(&name, id).await? {
            let _ = tokio::process::Command::new("docker")
                .args(["rm", "-f", &name])
                .output()
                .await;
            anyhow::bail!(
                "SANDBOX_CONTAINER_CREATE_FAILED: created container contract does not match expected contract (container={name})"
            );
        }

        Ok(())
    }

    async fn exec(&self, id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult> {
        let name = self.container_name(id);

        // Refresh the public data view so USER.md / PEOPLE.md reads inside sandbox are current.
        if self.config.data_mount_type == Some(DataMountType::Bind)
            && let Some(ref source) = self.config.data_mount_source
        {
            let _ = prepare_public_data_view(source, &id.key);
        }

        let mut args = vec!["exec".to_string()];

        if let Some(ref dir) = opts.working_dir {
            args.extend(["-w".to_string(), dir.display().to_string()]);
        }

        for (k, v) in &opts.env {
            args.extend(["-e".to_string(), format!("{k}={v}")]);
        }

        args.push(name);
        args.extend(["sh".to_string(), "-c".to_string(), command.to_string()]);

        let child = tokio::process::Command::new("docker")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .spawn()?;

        let result = tokio::time::timeout(opts.timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => Ok(ExecResult::from_process_output(
                output,
                opts.max_output_bytes,
            )),
            Ok(Err(e)) => anyhow::bail!("SANDBOX_EXEC_FAILED: docker exec failed: {e}"),
            Err(_) => anyhow::bail!(
                "SANDBOX_EXEC_FAILED: docker exec timed out after {}s",
                opts.timeout.as_secs()
            ),
        }
    }

    async fn cleanup(&self, id: &SandboxId) -> Result<()> {
        ensure_docker_daemon_available().await?;

        let name = self.container_name(id);
        let output = tokio::process::Command::new("docker")
            .args(["rm", "-f", &name])
            .output()
            .await?;
        if output.status.success() {
            // Best-effort cleanup of bind-view dir.
            if self.config.data_mount_type == Some(DataMountType::Bind)
                && let Some(ref source) = self.config.data_mount_source
            {
                let _ = remove_public_entry(&public_data_view_dir(source, &id.key));
            }
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        // Treat "No such container" as success.
        if stderr.contains("No such container") || stderr.contains("not found") {
            return Ok(());
        }
        anyhow::bail!(
            "SANDBOX_CONTAINER_DELETE_FAILED: docker rm -f failed for {name}: {}",
            stderr.trim()
        );
    }
}

/// No-op sandbox that passes through to direct execution.
pub struct NoSandbox;

#[async_trait]
impl Sandbox for NoSandbox {
    fn backend_name(&self) -> &'static str {
        "none"
    }

    async fn ensure_ready(&self, _id: &SandboxId) -> Result<()> {
        anyhow::bail!("SANDBOX_BACKEND_UNAVAILABLE: sandbox backend is disabled")
    }

    async fn exec(&self, _id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult> {
        let _ = (command, opts);
        anyhow::bail!("SANDBOX_BACKEND_UNAVAILABLE: sandbox backend is disabled")
    }

    async fn cleanup(&self, _id: &SandboxId) -> Result<()> {
        Ok(())
    }
}

fn sanitize_readable_fragment(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let compact = sanitized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if compact.is_empty() {
        "session".to_string()
    } else {
        compact
    }
}

fn sandbox_readable_slice(effective_key: &str) -> String {
    match moltis_sessions::SessionKey::parse(effective_key) {
        Ok(moltis_sessions::key::ParsedSessionKey::Agent { agent_id, bucket_key }) => {
            if bucket_key == "main" {
                return sanitize_readable_fragment(&format!("agent-{agent_id}-main"));
            }
            if bucket_key.starts_with("chat-") {
                return sanitize_readable_fragment(&format!("agent-{agent_id}-chat"));
            }
            if bucket_key.starts_with("dm-") {
                return sanitize_readable_fragment(&format!("agent-{agent_id}-dm"));
            }
            if let Some(chat_suffix) = bucket_key.strip_prefix("group-peer-tgchat.n") {
                let chat_id = chat_suffix
                    .split('-')
                    .next()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("group");
                return sanitize_readable_fragment(&format!("agent-{agent_id}-group-{chat_id}"));
            }
            sanitize_readable_fragment(&format!("agent-{agent_id}-{bucket_key}"))
        }
        Ok(moltis_sessions::key::ParsedSessionKey::System {
            service_id,
            bucket_key,
        }) => sanitize_readable_fragment(&format!("system-{service_id}-{bucket_key}")),
        Err(_) => sanitize_readable_fragment(effective_key),
    }
}

fn sandbox_runtime_name(effective_key: &str) -> String {
    let readable = sandbox_readable_slice(effective_key);
    let hash = Sha256::digest(effective_key.as_bytes());
    let short_hash = hash[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("msb-{readable}-{short_hash}")
}

/// In-process lease guard for a sandbox key.
///
/// Prevents TTL pruning from removing a sandbox while it is actively used.
pub struct SandboxLease {
    key: String,
    lease_counts: std::sync::Arc<std::sync::Mutex<HashMap<String, u32>>>,
}

impl Drop for SandboxLease {
    fn drop(&mut self) {
        let mut leases = self.lease_counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = leases.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                leases.remove(&self.key);
            }
        }
    }
}

/// Routes sandbox decisions per-session, based on global config (no per-session overrides).
pub struct SandboxRouter {
    config: SandboxConfig,
    backend: Arc<dyn Sandbox>,
    ensure_ready_locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    last_used_ms: std::sync::Mutex<HashMap<String, u64>>,
    lease_counts: std::sync::Arc<std::sync::Mutex<HashMap<String, u32>>>,
}

impl SandboxRouter {
    pub fn new(config: SandboxConfig) -> Self {
        let backend: Arc<dyn Sandbox> = match config.mode {
            SandboxMode::Off => Arc::new(NoSandbox),
            SandboxMode::All | SandboxMode::NonMain => Arc::new(DockerSandbox::new(config.clone())),
        };
        Self {
            config,
            backend,
            ensure_ready_locks: tokio::sync::Mutex::new(HashMap::new()),
            last_used_ms: std::sync::Mutex::new(HashMap::new()),
            lease_counts: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Create a router with a custom sandbox backend (testing only).
    pub fn with_backend(config: SandboxConfig, backend: Arc<dyn Sandbox>) -> Self {
        Self {
            config,
            backend,
            ensure_ready_locks: tokio::sync::Mutex::new(HashMap::new()),
            last_used_ms: std::sync::Mutex::new(HashMap::new()),
            lease_counts: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    pub fn mode(&self) -> &SandboxMode {
        &self.config.mode
    }

    pub fn backend(&self) -> &Arc<dyn Sandbox> {
        &self.backend
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.backend_name()
    }

    pub async fn startup_ensure_ready(&self) -> Result<()> {
        match self.config.mode {
            SandboxMode::Off => Ok(()),
            SandboxMode::All | SandboxMode::NonMain => DockerSandbox::new(self.config.clone())
                .startup_ensure_ready()
                .await,
        }
    }

    /// Check whether a session should run sandboxed.
    pub async fn is_sandboxed(
        &self,
        _session_id: &str,
        session_key: Option<&str>,
    ) -> Result<bool> {
        Ok(match self.config.mode {
            SandboxMode::Off => false,
            SandboxMode::All => true,
            SandboxMode::NonMain => {
                let canonical_session_key = session_key
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("missing session_key for sandbox mode non-main"))?;
                !matches!(
                    moltis_sessions::SessionKey::parse(canonical_session_key),
                    Ok(moltis_sessions::key::ParsedSessionKey::Agent { bucket_key, .. })
                        if bucket_key == "main"
                )
            }
        })
    }

    pub fn effective_sandbox_key(
        &self,
        session_id: &str,
        session_key: Option<&str>,
    ) -> Result<String> {
        match self.config.scope_key {
            SandboxScopeKey::SessionId => Ok(session_id.to_string()),
            SandboxScopeKey::SessionKey => {
                if let Some(key) = session_key.map(str::trim).filter(|s| !s.is_empty()) {
                    return Ok(key.to_string());
                }
                warn!(
                    reason_code = "missing_session_key_for_scope_key_session_key",
                    session_id,
                    "scope_key=session_key missing session_key; rejecting sandbox lookup"
                );
                anyhow::bail!("missing session_key for sandbox scope_key=session_key")
            }
        }
    }

    pub fn sandbox_id_for(&self, session_id: &str, session_key: Option<&str>) -> Result<SandboxId> {
        let effective_key = self.effective_sandbox_key(session_id, session_key)?;
        Ok(SandboxId {
            scope_key: self.config.scope_key.clone(),
            key: sandbox_runtime_name(&effective_key),
        })
    }

    fn touch_effective_key(&self, effective_key: &str) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut last = self.last_used_ms.lock().unwrap_or_else(|e| e.into_inner());
        last.insert(effective_key.to_string(), now_ms);
    }

    pub fn touch(&self, session_id: &str, session_key: Option<&str>) -> Result<()> {
        let effective = self.effective_sandbox_key(session_id, session_key)?;
        self.touch_effective_key(&effective);
        Ok(())
    }

    pub fn acquire_lease(
        &self,
        session_id: &str,
        session_key: Option<&str>,
    ) -> Result<SandboxLease> {
        let key = self.effective_sandbox_key(session_id, session_key)?;
        {
            let mut leases = self.lease_counts.lock().unwrap_or_else(|e| e.into_inner());
            *leases.entry(key.clone()).or_insert(0) += 1;
        }
        Ok(SandboxLease {
            key,
            lease_counts: std::sync::Arc::clone(&self.lease_counts),
        })
    }

    async fn lock_for_effective_key(&self, effective_key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.ensure_ready_locks.lock().await;
        locks
            .entry(effective_key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub async fn ensure_ready_for_session(
        &self,
        session_id: &str,
        session_key: Option<&str>,
    ) -> Result<SandboxId> {
        let sandboxed = self.is_sandboxed(session_id, session_key).await?;
        if !sandboxed {
            anyhow::bail!("SANDBOX_MODE_OFF: session is not sandboxed under current mode");
        }

        let effective_key = self.effective_sandbox_key(session_id, session_key)?;
        let lock = self.lock_for_effective_key(&effective_key).await;
        let _guard = lock.lock().await;
        let id = self.sandbox_id_for(session_id, session_key)?;
        self.backend.ensure_ready(&id).await?;
        Ok(id)
    }

    pub async fn cleanup_effective_key(&self, effective_key: &str) -> Result<()> {
        let lock = self.lock_for_effective_key(effective_key).await;
        let _guard = lock.lock().await;
        let id = SandboxId {
            scope_key: self.config.scope_key.clone(),
            key: sandbox_runtime_name(effective_key),
        };
        self.backend.cleanup(&id).await?;
        {
            let mut last = self.last_used_ms.lock().unwrap_or_else(|e| e.into_inner());
            last.remove(effective_key);
        }
        {
            let mut locks = self.ensure_ready_locks.lock().await;
            locks.remove(effective_key);
        }
        Ok(())
    }

    pub async fn cleanup_session(&self, session_id: &str, session_key: Option<&str>) -> Result<()> {
        if self.config.scope_key != SandboxScopeKey::SessionId {
            return Ok(());
        }
        let sandboxed = self.is_sandboxed(session_id, session_key).await?;
        if !sandboxed {
            return Ok(());
        }

        let id = self.sandbox_id_for(session_id, session_key)?;
        let lock = self.lock_for_effective_key(session_id).await;
        let _guard = lock.lock().await;
        self.backend.cleanup(&id).await?;
        {
            let mut locks = self.ensure_ready_locks.lock().await;
            locks.remove(session_id);
        }
        Ok(())
    }

    pub async fn prune_idle(&self) {
        let ttl = self.config.idle_ttl_secs;
        if ttl == 0 {
            return;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let ttl_ms = ttl.saturating_mul(1000);

        let mut candidates = Vec::new();
        {
            let last = self.last_used_ms.lock().unwrap_or_else(|e| e.into_inner());
            for (key, ts) in last.iter() {
                if now_ms.saturating_sub(*ts) >= ttl_ms {
                    candidates.push(key.clone());
                }
            }
        }

        for effective_key in candidates {
            let in_use = {
                let leases = self.lease_counts.lock().unwrap_or_else(|e| e.into_inner());
                leases.get(&effective_key).copied().unwrap_or(0) > 0
            };
            if in_use {
                continue;
            }

            let lock = self.lock_for_effective_key(&effective_key).await;
            let Ok(_guard) = lock.try_lock() else {
                continue;
            };

            let id = SandboxId {
                scope_key: self.config.scope_key.clone(),
                key: sandbox_runtime_name(&effective_key),
            };
            if let Err(e) = self.backend.cleanup(&id).await {
                warn!(effective_key, error = %e, "sandbox prune failed");
                continue;
            }

            {
                let mut last = self.last_used_ms.lock().unwrap_or_else(|e| e.into_inner());
                last.remove(&effective_key);
            }
            {
                let mut locks = self.ensure_ready_locks.lock().await;
                locks.remove(&effective_key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_runtime_name_stable_prefix() {
        let name = sandbox_runtime_name("agent:abc/main");
        assert!(name.starts_with("msb-"));
    }

    #[test]
    fn test_docker_run_args_workdir_env_labels() {
        let mut cfg = SandboxConfig::default();
        cfg.image = Some("moltis-sandbox:test".into());
        cfg.mode = SandboxMode::All;
        cfg.data_mount = WorkspaceMount::Ro;
        cfg.data_mount_type = Some(DataMountType::Volume);
        cfg.data_mount_source = Some("moltis-data".into());

        let docker = DockerSandbox::new(cfg);
        let id = SandboxId {
            scope_key: SandboxScopeKey::SessionId,
            key: "msb-test-00000000".into(),
        };
        let args = docker
            .docker_run_args("msb-test-00000000", "moltis-sandbox:test", &id)
            .unwrap();

        assert!(args.iter().any(|v| v == "-w"));
        assert!(args.iter().any(|v| v == SANDBOX_GUEST_WORKDIR));
        assert!(args
            .iter()
            .any(|v| v == &format!("HOME={SANDBOX_GUEST_WORKDIR}")));
        assert!(args
            .iter()
            .any(|v| v == &format!("TMPDIR={SANDBOX_GUEST_TMPDIR}")));
        assert!(args
            .iter()
            .any(|v| v == &format!("{SANDBOX_LABEL_CONTRACT_VERSION}={SANDBOX_CONTAINER_CONTRACT_VERSION}")));
    }

    #[test]
    fn test_schema_try_from_rejects_legacy_fields() {
        let mut cfg = moltis_config::schema::SandboxConfig::default();
        cfg.mode = "all".into();
        cfg.scope_key = "session_id".into();
        cfg.data_mount = "ro".into();
        cfg.data_mount_type = Some("volume".into());
        cfg.data_mount_source = Some("moltis-data".into());
        cfg.image = Some("moltis-sandbox:test".into());

        cfg.backend = Some("auto".into());
        let err = SandboxConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_LEGACY_BACKEND_REMOVED"));

        cfg.backend = None;
        cfg.packages = Some(vec!["curl".into()]);
        let err = SandboxConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_LEGACY_BUILD_PATH_REMOVED"));

        cfg.packages = None;
        cfg.container_prefix = Some("msb".into());
        let err = SandboxConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_LEGACY_CONTAINER_PREFIX_REMOVED"));

        cfg.container_prefix = None;
        cfg.scope = Some("session".into());
        let err = SandboxConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_LEGACY_SCOPE_REMOVED"));
    }

    #[test]
    fn test_schema_try_from_rejects_non_main_alias() {
        let mut cfg = moltis_config::schema::SandboxConfig::default();
        cfg.mode = "non_main".into();
        cfg.scope_key = "session_id".into();
        cfg.data_mount = "ro".into();
        cfg.data_mount_type = Some("volume".into());
        cfg.data_mount_source = Some("moltis-data".into());
        cfg.image = Some("moltis-sandbox:test".into());

        let err = SandboxConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_CONFIG_INVALID: unknown sandbox mode"));
    }

    #[test]
    fn test_schema_try_from_requires_image_when_enabled() {
        let mut cfg = moltis_config::schema::SandboxConfig::default();
        cfg.mode = "all".into();
        cfg.scope_key = "session_id".into();
        cfg.data_mount = "ro".into();
        cfg.data_mount_type = Some("volume".into());
        cfg.data_mount_source = Some("moltis-data".into());
        cfg.image = None;

        let err = SandboxConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_IMAGE_MISSING"));
    }

    #[tokio::test]
    async fn test_router_ensure_ready_serialized_per_key() {
        #[derive(Default)]
        struct FakeSandbox {
            calls: tokio::sync::Mutex<u32>,
        }

        #[async_trait]
        impl Sandbox for FakeSandbox {
            fn backend_name(&self) -> &'static str {
                "fake"
            }

            async fn ensure_ready(&self, _id: &SandboxId) -> Result<()> {
                let mut n = self.calls.lock().await;
                *n += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                Ok(())
            }

            async fn exec(
                &self,
                _id: &SandboxId,
                _command: &str,
                _opts: &ExecOpts,
            ) -> Result<ExecResult> {
                unreachable!()
            }

            async fn cleanup(&self, _id: &SandboxId) -> Result<()> {
                Ok(())
            }
        }

        let mut cfg = SandboxConfig::default();
        cfg.mode = SandboxMode::All;
        cfg.image = Some("moltis-sandbox:test".into());
        cfg.data_mount = WorkspaceMount::Ro;
        cfg.data_mount_type = Some(DataMountType::Volume);
        cfg.data_mount_source = Some("moltis-data".into());

        let backend = Arc::new(FakeSandbox::default());
        let router = Arc::new(SandboxRouter::with_backend(cfg, backend.clone()));

        let fut1 = router.ensure_ready_for_session("s1", Some("agent:abc/main"));
        let fut2 = router.ensure_ready_for_session("s1", Some("agent:abc/main"));
        let _ = tokio::join!(fut1, fut2);

        let calls = *backend.calls.lock().await;
        assert_eq!(calls, 2);
    }
}
