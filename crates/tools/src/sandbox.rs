use std::{collections::HashMap, sync::Arc};

use {
    anyhow::{Context, Result},
    async_trait::async_trait,
    serde::{Deserialize, Serialize},
    tokio::sync::RwLock,
    tracing::{debug, info, warn},
};

use crate::exec::{ExecOpts, ExecResult};

/// Install configured packages inside a container via `apt-get`.
///
/// `cli` is the container CLI binary name (e.g. `"docker"` or `"container"`).
async fn provision_packages(cli: &str, container_name: &str, packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    let pkg_list = packages.join(" ");
    info!(container = container_name, packages = %pkg_list, "provisioning sandbox packages");
    let output = tokio::process::Command::new(cli)
        .args([
            "exec",
            container_name,
            "sh",
            "-c",
            &format!("apt-get update -qq && apt-get install -y -qq {pkg_list} 2>&1 | tail -5"),
        ])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            container = container_name,
            %stderr,
            "package provisioning failed (non-fatal)"
        );
    }
    Ok(())
}

/// Check whether the current process is running as root (UID 0).
fn is_running_as_root() -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("id")
            .args(["-u"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .is_some_and(|uid| uid.trim() == "0")
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Check whether the current host is Debian/Ubuntu (has `/etc/debian_version`
/// and `apt-get` on PATH).
pub fn is_debian_host() -> bool {
    std::path::Path::new("/etc/debian_version").exists() && is_cli_available("apt-get")
}

fn host_package_name_candidates(pkg: &str) -> Vec<String> {
    let mut candidates = vec![pkg.to_string()];

    if let Some(base) = pkg.strip_suffix("t64") {
        candidates.push(base.to_string());
        return candidates;
    }

    let looks_like_soname_package =
        pkg.starts_with("lib") && pkg.chars().last().is_some_and(|c| c.is_ascii_digit());
    if looks_like_soname_package {
        candidates.push(format!("{pkg}t64"));
    }

    candidates
}

async fn is_installed_dpkg_package(pkg: &str) -> bool {
    tokio::process::Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", pkg])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .is_ok_and(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).contains("install ok installed")
        })
}

async fn resolve_installed_host_package(pkg: &str) -> Option<String> {
    for candidate in host_package_name_candidates(pkg) {
        if is_installed_dpkg_package(&candidate).await {
            return Some(candidate);
        }
    }
    None
}

async fn is_apt_package_available(pkg: &str) -> bool {
    tokio::process::Command::new("apt-cache")
        .args(["show", pkg])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

async fn resolve_installable_host_package(pkg: &str) -> Option<String> {
    for candidate in host_package_name_candidates(pkg) {
        if is_apt_package_available(&candidate).await {
            return Some(candidate);
        }
    }
    None
}

/// Result of host package provisioning.
#[derive(Debug, Clone)]
pub struct HostProvisionResult {
    /// Packages that were actually installed.
    pub installed: Vec<String>,
    /// Packages that were already present.
    pub skipped: Vec<String>,
    /// Whether sudo was used for installation.
    pub used_sudo: bool,
}

/// Install configured packages directly on the host via `apt-get`.
///
/// Used when the sandbox backend is `"none"` (no container runtime) and the
/// host is Debian/Ubuntu. Returns `None` if packages are empty or the host
/// is not Debian-based.
///
/// This is **non-fatal**: failures are logged as warnings and do not block
/// startup.
pub async fn provision_host_packages(packages: &[String]) -> Result<Option<HostProvisionResult>> {
    if packages.is_empty() || !is_debian_host() {
        return Ok(None);
    }

    // Determine which packages are already installed via dpkg-query.
    let mut missing = Vec::new();
    let mut skipped = Vec::new();

    for pkg in packages {
        if resolve_installed_host_package(pkg).await.is_some() {
            skipped.push(pkg.clone());
        } else {
            missing.push(pkg.clone());
        }
    }

    if missing.is_empty() {
        info!(
            skipped = skipped.len(),
            "all host packages already installed"
        );
        return Ok(Some(HostProvisionResult {
            installed: Vec::new(),
            skipped,
            used_sudo: false,
        }));
    }

    // Check if we can use sudo without a password prompt.
    let has_sudo = tokio::process::Command::new("sudo")
        .args(["-n", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());

    let is_root = is_running_as_root();

    if !has_sudo && !is_root {
        info!(
            missing = missing.len(),
            "not running as root and passwordless sudo unavailable; \
             skipping host package provisioning (install packages in the container image instead)"
        );
        return Ok(Some(HostProvisionResult {
            installed: Vec::new(),
            skipped: missing,
            used_sudo: false,
        }));
    }

    let apt_update = if has_sudo {
        "sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq".to_string()
    } else {
        "DEBIAN_FRONTEND=noninteractive apt-get update -qq".to_string()
    };

    // Run apt-get update.
    let update_out = tokio::process::Command::new("sh")
        .args(["-c", &apt_update])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;
    if let Ok(ref out) = update_out
        && !out.status.success()
    {
        let stderr = String::from_utf8_lossy(&out.stderr);
        warn!(%stderr, "apt-get update failed (non-fatal)");
    }

    // Resolve distro-specific package aliases after apt metadata is refreshed.
    let mut installable = Vec::new();
    let mut remapped = Vec::new();
    let mut unavailable = Vec::new();
    for pkg in &missing {
        match resolve_installable_host_package(pkg).await {
            Some(host_pkg) => {
                if host_pkg != *pkg {
                    remapped.push(format!("{pkg}->{host_pkg}"));
                }
                installable.push(host_pkg);
            },
            None => unavailable.push(pkg.clone()),
        }
    }
    installable.sort_unstable();
    installable.dedup();

    if !remapped.is_empty() {
        info!(
            count = remapped.len(),
            remapped = %remapped.join(", "),
            "resolved distro-specific package aliases for host provisioning"
        );
    }
    if !unavailable.is_empty() {
        warn!(
            packages = %unavailable.join(" "),
            "host package(s) unavailable on this distro; skipping"
        );
        skipped.extend(unavailable);
    }
    if installable.is_empty() {
        info!(
            skipped = skipped.len(),
            "no installable host packages after distro compatibility resolution"
        );
        return Ok(Some(HostProvisionResult {
            installed: Vec::new(),
            skipped,
            used_sudo: has_sudo,
        }));
    }

    let pkg_list = installable.join(" ");
    let apt_install = if has_sudo {
        format!("sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq {pkg_list}")
    } else {
        format!("DEBIAN_FRONTEND=noninteractive apt-get install -y -qq {pkg_list}")
    };

    info!(
        packages = %pkg_list,
        sudo = has_sudo,
        "provisioning host packages"
    );

    // Run apt-get install.
    let install_out = tokio::process::Command::new("sh")
        .args(["-c", &apt_install])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match install_out {
        Ok(out) if out.status.success() => {
            info!(
                installed = installable.len(),
                skipped = skipped.len(),
                "host packages provisioned"
            );
            Ok(Some(HostProvisionResult {
                installed: installable,
                skipped,
                used_sudo: has_sudo,
            }))
        },
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            warn!(
                %stderr,
                "apt-get install failed (non-fatal)"
            );
            Ok(Some(HostProvisionResult {
                installed: Vec::new(),
                skipped,
                used_sudo: has_sudo,
            }))
        },
        Err(e) => {
            warn!(%e, "failed to run apt-get install (non-fatal)");
            Ok(Some(HostProvisionResult {
                installed: Vec::new(),
                skipped,
                used_sudo: has_sudo,
            }))
        },
    }
}

/// Default container image used when none is configured.
pub const DEFAULT_SANDBOX_IMAGE: &str = "ubuntu:25.10";

/// Fixed guest mountpoint for Moltis instance data inside sandbox containers.
pub const SANDBOX_GUEST_DATA_DIR: &str = "/moltis/data";

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

fn prepare_public_data_view(base_data_dir: &str, sandbox_key: &str) -> anyhow::Result<std::path::PathBuf> {
    let view_dir = public_data_view_dir(base_data_dir, sandbox_key);
    std::fs::create_dir_all(&view_dir)?;

    // Only expose public workspace files to sandboxed exec.
    let base = std::path::PathBuf::from(base_data_dir);
    copy_file_or_empty(&base.join("USER.md"), &view_dir.join("USER.md"))?;
    copy_file_or_empty(&base.join("PEOPLE.md"), &view_dir.join("PEOPLE.md"))?;

    Ok(view_dir)
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

/// Scope determines sandbox container reuse boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxScope {
    Session,
    /// Reuse sandbox per chat (e.g. a Telegram group).
    Chat,
    /// Reuse sandbox per bot/account.
    Bot,
    /// Single shared sandbox for the whole instance.
    Global,
}

impl Default for SandboxScope {
    fn default() -> Self {
        Self::Chat
    }
}

impl std::fmt::Display for SandboxScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session => f.write_str("session"),
            Self::Chat => f.write_str("chat"),
            Self::Bot => f.write_str("bot"),
            Self::Global => f.write_str("global"),
        }
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

/// Configuration for sandbox behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    pub scope: SandboxScope,
    /// Idle TTL for sandbox containers (seconds).
    ///
    /// - `>0`: idle containers may be reclaimed after TTL.
    /// - `=0`: disable TTL; containers persist unless cleaned by other policy.
    pub idle_ttl_secs: u64,
    /// Whether to mount the Moltis data directory into sandbox containers.
    ///
    /// When enabled, Docker mounts `data_mount_source` to the fixed guest path
    /// `/moltis/data` (ro/rw based on this value).
    pub data_mount: WorkspaceMount,
    pub data_mount_type: Option<DataMountType>,
    pub data_mount_source: Option<String>,
    /// Additional host directory mounts exposed inside the sandbox container.
    ///
    /// Deny-by-default: mounts require `mount_allowlist` entries and are validated
    /// before container creation.
    #[serde(default)]
    pub mounts: Vec<SandboxMount>,
    /// Allowlist of host directory roots permitted for `mounts[*].host_dir`.
    #[serde(default)]
    pub mount_allowlist: Vec<std::path::PathBuf>,
    pub image: Option<String>,
    pub container_prefix: Option<String>,
    pub no_network: bool,
    /// Backend: `"auto"` (default) or `"docker"`.
    /// `"auto"` uses Docker when available, falls back to direct execution.
    ///
    /// Note: `"apple-container"` is intentionally not supported (will fail-fast).
    pub backend: String,
    pub resource_limits: ResourceLimits,
    /// Packages to install via `apt-get` after container creation.
    /// Set to an empty list to skip provisioning.
    pub packages: Vec<String>,
    /// IANA timezone (e.g. "Europe/Paris") injected as `TZ` env var into containers.
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
            mode: SandboxMode::default(),
            scope: SandboxScope::default(),
            idle_ttl_secs: 0,
            data_mount: WorkspaceMount::default(),
            data_mount_type: None,
            data_mount_source: None,
            mounts: Vec::new(),
            mount_allowlist: Vec::new(),
            image: None,
            container_prefix: None,
            no_network: false,
            backend: "auto".into(),
            resource_limits: ResourceLimits::default(),
            packages: Vec::new(),
            timezone: None,
        }
    }
}

impl From<&moltis_config::schema::SandboxConfig> for SandboxConfig {
    fn from(cfg: &moltis_config::schema::SandboxConfig) -> Self {
        let scope_raw = cfg.scope.as_str();
        Self {
            mode: match cfg.mode.as_str() {
                "all" => SandboxMode::All,
                "non-main" | "nonmain" => SandboxMode::NonMain,
                _ => SandboxMode::Off,
            },
            scope: match scope_raw {
                "session" => SandboxScope::Session,
                "chat" => SandboxScope::Chat,
                "bot" => SandboxScope::Bot,
                "global" => SandboxScope::Global,
                _ => {
                    warn!(
                        scope = scope_raw,
                        "unknown tools.exec.sandbox.scope; falling back to session"
                    );
                    SandboxScope::Session
                },
            },
            idle_ttl_secs: cfg.idle_ttl_secs,
            data_mount: match cfg.data_mount.as_str() {
                "rw" => WorkspaceMount::Rw,
                "none" => WorkspaceMount::None,
                _ => WorkspaceMount::Ro,
            },
            data_mount_type: cfg.data_mount_type.as_deref().and_then(|raw| match raw {
                "bind" => Some(DataMountType::Bind),
                "volume" => Some(DataMountType::Volume),
                _ => None,
            }),
            data_mount_source: cfg.data_mount_source.clone(),
            mounts: cfg
                .mounts
                .iter()
                .map(|m| SandboxMount {
                    host_dir: std::path::PathBuf::from(&m.host_dir),
                    guest_dir: std::path::PathBuf::from(&m.guest_dir),
                    mode: match m.mode.as_str() {
                        "rw" => WorkspaceMount::Rw,
                        _ => WorkspaceMount::Ro,
                    },
                })
                .collect(),
            mount_allowlist: cfg
                .mount_allowlist
                .iter()
                .map(std::path::PathBuf::from)
                .collect(),
            image: cfg.image.clone(),
            container_prefix: cfg.container_prefix.clone(),
            no_network: cfg.no_network,
            backend: cfg.backend.clone(),
            resource_limits: ResourceLimits {
                memory_limit: cfg.resource_limits.memory_limit.clone(),
                cpu_quota: cfg.resource_limits.cpu_quota,
                pids_max: cfg.resource_limits.pids_max,
            },
            packages: cfg.packages.clone(),
            timezone: None, // Set by gateway from user profile
        }
    }
}

/// Sandbox identifier — session or agent scoped.
#[derive(Debug, Clone)]
pub struct SandboxId {
    pub scope: SandboxScope,
    pub key: String,
}

impl std::fmt::Display for SandboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}/{}", self.scope, self.key)
    }
}

/// Result of a `build_image` call.
#[derive(Debug, Clone)]
pub struct BuildImageResult {
    /// The full image tag (e.g. `moltis-sandbox:abc123`).
    pub tag: String,
    /// Whether the build was actually performed (false = image already existed).
    pub built: bool,
}

/// Trait for sandbox implementations (Docker, cgroups, Apple Container, etc.).
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Human-readable backend name (e.g. "docker", "apple-container", "cgroup", "none").
    fn backend_name(&self) -> &'static str;

    /// Ensure the sandbox environment is ready (e.g., container started).
    /// If `image_override` is provided, use that image instead of the configured default.
    async fn ensure_ready(&self, id: &SandboxId, image_override: Option<&str>) -> Result<()>;

    /// Execute a command inside the sandbox.
    async fn exec(&self, id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult>;

    /// Clean up sandbox resources.
    async fn cleanup(&self, id: &SandboxId) -> Result<()>;

    /// Pre-build a container image with packages baked in.
    /// Returns `None` for backends that don't support image building.
    async fn build_image(
        &self,
        _base: &str,
        _packages: &[String],
    ) -> Result<Option<BuildImageResult>> {
        Ok(None)
    }
}

/// Compute the content-hash tag for a pre-built sandbox image.
/// Pure function — independent of any specific container CLI.
pub fn sandbox_image_tag(repo: &str, base: &str, packages: &[String]) -> String {
    use std::hash::Hasher;
    let mut h = std::hash::DefaultHasher::new();
    // Bump this when the Dockerfile template changes to force a rebuild.
    h.write(b"v4");
    h.write(repo.as_bytes());
    h.write(base.as_bytes());
    let mut sorted: Vec<&String> = packages.iter().collect();
    sorted.sort();
    for p in &sorted {
        h.write(p.as_bytes());
    }
    format!("{repo}:{:016x}", h.finish())
}

fn is_sandbox_image_tag(tag: &str) -> bool {
    let Some((repo, _)) = tag.split_once(':') else {
        return false;
    };
    repo.ends_with("-sandbox")
}

/// Check whether a container image exists locally.
/// `cli` is the container CLI binary (e.g. `"docker"` or `"container"`).
async fn sandbox_image_exists(cli: &str, tag: &str) -> bool {
    tokio::process::Command::new(cli)
        .args(["image", "inspect", tag])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

/// Information about a locally cached sandbox image.
#[derive(Debug, Clone)]
pub struct SandboxImage {
    pub tag: String,
    pub size: String,
    pub created: String,
}

/// List all local `<instance>-sandbox:*` images across available container CLIs.
pub async fn list_sandbox_images() -> Result<Vec<SandboxImage>> {
    let mut images = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Docker: supports --format with Go templates.
    if is_cli_available("docker") {
        let output = tokio::process::Command::new("docker")
            .args([
                "image",
                "ls",
                "--format",
                "{{.Repository}}:{{.Tag}}\t{{.Size}}\t{{.CreatedSince}}",
            ])
            .output()
            .await?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.splitn(3, '\t').collect();
                if parts.len() == 3
                    && is_sandbox_image_tag(parts[0])
                    && seen.insert(parts[0].to_string())
                {
                    images.push(SandboxImage {
                        tag: parts[0].to_string(),
                        size: parts[1].to_string(),
                        created: parts[2].to_string(),
                    });
                }
            }
        }
    }

    // Apple Container: fixed table output (NAME  TAG  DIGEST), no --format.
    // Parse the table, then use `image inspect` JSON for metadata.
    if is_cli_available("container") {
        let output = tokio::process::Command::new("container")
            .args(["image", "ls"])
            .output()
            .await?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().skip(1) {
                // Columns are whitespace-separated: NAME TAG DIGEST
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() >= 2 && cols[0].ends_with("-sandbox") {
                    let tag = format!("{}:{}", cols[0], cols[1]);
                    if !seen.insert(tag.clone()) {
                        continue;
                    }
                    // Fetch size and created from inspect JSON.
                    let (size, created) = inspect_apple_container_image(&tag).await;
                    images.push(SandboxImage { tag, size, created });
                }
            }
        }
    }

    Ok(images)
}

/// Extract size and created timestamp from Apple Container `image inspect` JSON.
async fn inspect_apple_container_image(tag: &str) -> (String, String) {
    let output = tokio::process::Command::new("container")
        .args(["image", "inspect", tag])
        .output()
        .await;
    let fallback = ("—".to_string(), "—".to_string());
    let Ok(output) = output else {
        return fallback;
    };
    if !output.status.success() {
        return fallback;
    }
    let Ok(json): std::result::Result<serde_json::Value, _> =
        serde_json::from_slice(&output.stdout)
    else {
        return fallback;
    };
    let entry = json.as_array().and_then(|a| a.first());
    let Some(entry) = entry else {
        return fallback;
    };
    let created = entry
        .pointer("/index/annotations/org.opencontainers.image.created")
        .and_then(|v| v.as_str())
        .unwrap_or("—")
        .to_string();
    let size = entry
        .pointer("/variants/0/size")
        .and_then(|v| v.as_u64())
        .map(format_bytes)
        .unwrap_or_else(|| "—".to_string());
    (size, created)
}

/// Format a byte count as a human-readable string (e.g. "361 MB").
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Remove a specific `<instance>-sandbox:*` image.
pub async fn remove_sandbox_image(tag: &str) -> Result<()> {
    anyhow::ensure!(
        is_sandbox_image_tag(tag),
        "refusing to remove non-sandbox image: {tag}"
    );
    for cli in &["docker", "container"] {
        if !is_cli_available(cli) {
            continue;
        }
        if sandbox_image_exists(cli, tag).await {
            // Apple Container uses `image delete`, Docker uses `image rm`.
            let subcmd = if *cli == "container" {
                "delete"
            } else {
                "rm"
            };
            let output = tokio::process::Command::new(cli)
                .args(["image", subcmd, tag])
                .output()
                .await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("{cli} image {subcmd} failed for {tag}: {}", stderr.trim());
            }
        }
    }
    Ok(())
}

/// Remove all local `<instance>-sandbox:*` images.
pub async fn clean_sandbox_images() -> Result<usize> {
    let images = list_sandbox_images().await?;
    let count = images.len();
    for img in &images {
        remove_sandbox_image(&img.tag).await?;
    }
    Ok(count)
}

/// Docker-based sandbox implementation.
pub struct DockerSandbox {
    pub config: SandboxConfig,
}

impl DockerSandbox {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    fn normalize_bind_mount_source_for_compare(source: &str) -> String {
        // Lexically normalize absolute UNIX-style paths for comparison against
        // `docker inspect` output, without touching the filesystem.
        //
        // This reduces false contract mismatches when users provide paths with
        // redundant separators or dot segments.
        if !source.starts_with('/') {
            return source.trim().to_string();
        }

        let mut stack: Vec<&str> = Vec::new();
        for segment in source.split('/') {
            match segment {
                "" | "." => {},
                ".." => {
                    let _ = stack.pop();
                },
                other => stack.push(other),
            }
        }

        if stack.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", stack.join("/"))
        }
    }

    fn image(&self) -> &str {
        self.config
            .image
            .as_deref()
            .unwrap_or(DEFAULT_SANDBOX_IMAGE)
    }

    fn container_prefix(&self) -> &str {
        self.config
            .container_prefix
            .as_deref()
            .unwrap_or("moltis-sandbox")
    }

    fn container_name(&self, id: &SandboxId) -> String {
        format!("{}-{}", self.container_prefix(), id.key)
    }

    fn image_repo(&self) -> &str {
        self.container_prefix()
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
                    "SANDBOX_DATA_MOUNT_REQUIRED: docker sandbox requires data_dir mount; \
                     set tools.exec.sandbox.data_mount=ro|rw and set \
                     tools.exec.sandbox.data_mount_type/tools.exec.sandbox.data_mount_source"
                )
            },
        };

        let mount_type = self.config.data_mount_type.ok_or_else(|| {
            anyhow::anyhow!(
                "SANDBOX_DATA_MOUNT_REQUIRED: docker sandbox requires data_dir mount; \
                 set tools.exec.sandbox.data_mount=ro|rw and set \
                 tools.exec.sandbox.data_mount_type/tools.exec.sandbox.data_mount_source"
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
                    "SANDBOX_DATA_MOUNT_REQUIRED: docker sandbox requires data_dir mount; \
                     set tools.exec.sandbox.data_mount=ro|rw and set \
                     tools.exec.sandbox.data_mount_type/tools.exec.sandbox.data_mount_source"
                )
            })?;

        let mount_source_to_use = match mount_type {
            DataMountType::Bind => {
                if !std::path::Path::new(mount_source).is_absolute() {
                    anyhow::bail!(
                        "SANDBOX_DATA_MOUNT_INVALID: tools.exec.sandbox.data_mount_source must be an \
                         absolute path when tools.exec.sandbox.data_mount_type=bind"
                    );
                }
                if mount_source.contains(':') {
                    anyhow::bail!(
                        "SANDBOX_DATA_MOUNT_INVALID: tools.exec.sandbox.data_mount_source must not contain ':' \
                         when tools.exec.sandbox.data_mount_type=bind"
                    );
                }
                let view_dir = prepare_public_data_view(mount_source, &id.key)?;
                view_dir.display().to_string()
            },
            DataMountType::Volume => {
                if mount_source.contains('/')
                    || mount_source.contains('\\')
                    || mount_source.contains(':')
                    || mount_source.chars().any(char::is_whitespace)
                {
                    anyhow::bail!(
                        "SANDBOX_DATA_MOUNT_INVALID: tools.exec.sandbox.data_mount_source must be a \
                         Docker volume name when tools.exec.sandbox.data_mount_type=volume"
                    );
                }
                mount_source.to_string()
            },
        };

        Ok(vec![
            "-v".to_string(),
            format!("{mount_source_to_use}:{SANDBOX_GUEST_DATA_DIR}:{mode}"),
        ])
    }

    async fn container_contract_matches(&self, name: &str, id: &SandboxId) -> Result<bool> {
        let expected_env = format!("MOLTIS_DATA_DIR={SANDBOX_GUEST_DATA_DIR}");

        let env_output = tokio::process::Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{range .Config.Env}}{{println .}}{{end}}",
                name,
            ])
            .output()
            .await?;
        if !env_output.status.success() {
            return Ok(false);
        }
        let env_stdout = String::from_utf8_lossy(&env_output.stdout);
        let env_ok = env_stdout.lines().any(|l| l.trim() == expected_env);

        let mounts_output = tokio::process::Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{range .Mounts}}{{println .Type \"|\" .Name \"|\" .Source \"|\" .Destination \"|\" .RW}}{{end}}",
                name,
            ])
            .output()
            .await?;
        if !mounts_output.status.success() {
            return Ok(false);
        }
        let mounts_stdout = String::from_utf8_lossy(&mounts_output.stdout);

        let expected_mount_type = self.config.data_mount_type.ok_or_else(|| {
            anyhow::anyhow!(
                "SANDBOX_DATA_MOUNT_REQUIRED: docker sandbox requires data_dir mount; \
                 set tools.exec.sandbox.data_mount=ro|rw and set \
                 tools.exec.sandbox.data_mount_type/tools.exec.sandbox.data_mount_source"
            )
        })?;
        let expected_mount_source = self
            .config
            .data_mount_source
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "SANDBOX_DATA_MOUNT_REQUIRED: docker sandbox requires data_dir mount; \
                     set tools.exec.sandbox.data_mount=ro|rw and set \
                     tools.exec.sandbox.data_mount_type/tools.exec.sandbox.data_mount_source"
                )
            })?;
        let expected_mount_source_normalized = match expected_mount_type {
            DataMountType::Bind => {
                let view_dir =
                    public_data_view_dir(expected_mount_source, &id.key).display().to_string();
                Self::normalize_bind_mount_source_for_compare(&view_dir)
            },
            DataMountType::Volume => expected_mount_source.to_string(),
        };
        let expected_rw = match self.config.data_mount {
            WorkspaceMount::Rw => "true",
            WorkspaceMount::Ro => "false",
            WorkspaceMount::None => "false",
        };

        let mut mount_ok = false;
        for line in mounts_stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            let parts: Vec<&str> = line.split('|').map(str::trim).collect();
            if parts.len() != 5 {
                continue;
            }
            let mount_type = parts[0];
            let mount_name = parts[1];
            let mount_source = parts[2];
            let mount_dest = parts[3];
            let mount_rw = parts[4];

            if mount_dest != SANDBOX_GUEST_DATA_DIR {
                continue;
            }

            match expected_mount_type {
                DataMountType::Bind => {
                    let mount_source_normalized =
                        Self::normalize_bind_mount_source_for_compare(mount_source);
                    if mount_type == "bind"
                        && mount_source_normalized == expected_mount_source_normalized
                        && mount_rw == expected_rw
                    {
                        mount_ok = true;
                    }
                },
                DataMountType::Volume => {
                    if mount_type == "volume"
                        && mount_name == expected_mount_source_normalized
                        && mount_rw == expected_rw
                    {
                        mount_ok = true;
                    }
                },
            }
        }

        Ok(env_ok && mount_ok)
    }

    fn external_mount_args(&self) -> Result<Vec<String>> {
        const GUEST_PREFIX: &str = "/mnt/host/";

        if self.config.mounts.is_empty() {
            return Ok(Vec::new());
        }
        if self.config.mount_allowlist.is_empty() {
            anyhow::bail!(
                "sandbox mounts are configured but mount_allowlist is empty (deny-by-default)"
            );
        }

        let mut allow_roots = Vec::with_capacity(self.config.mount_allowlist.len());
        for root in &self.config.mount_allowlist {
            if !root.is_absolute() {
                anyhow::bail!(
                    "sandbox mount_allowlist entry must be an absolute path: {}",
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
                    "sandbox mount_allowlist entry must be a directory: {}",
                    canonical.display()
                );
            }
            allow_roots.push(canonical);
        }

        let mut args = Vec::new();
        for (i, mount) in self.config.mounts.iter().enumerate() {
            if mount.host_dir.as_os_str().is_empty() {
                anyhow::bail!("sandbox mounts[{i}].host_dir is empty");
            }
            if !mount.host_dir.is_absolute() {
                anyhow::bail!(
                    "sandbox mounts[{i}].host_dir must be an absolute path: {}",
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
                    "sandbox mounts[{i}].host_dir must be a directory: {}",
                    canonical_host.display()
                );
            }
            if !allow_roots
                .iter()
                .any(|root| canonical_host.starts_with(root))
            {
                anyhow::bail!(
                    "sandbox mounts[{i}].host_dir is outside mount_allowlist: {}",
                    canonical_host.display()
                );
            }

            if mount.guest_dir.as_os_str().is_empty() {
                anyhow::bail!("sandbox mounts[{i}].guest_dir is empty");
            }
            if !mount.guest_dir.is_absolute() {
                anyhow::bail!(
                    "sandbox mounts[{i}].guest_dir must be an absolute path: {}",
                    mount.guest_dir.display()
                );
            }
            let guest_str = mount.guest_dir.display().to_string();
            if !guest_str.starts_with(GUEST_PREFIX) {
                anyhow::bail!(
                    "sandbox mounts[{i}].guest_dir must be under {GUEST_PREFIX} (got: {guest_str})"
                );
            }
            if guest_str == "/"
                || guest_str == "/proc"
                || guest_str == "/sys"
                || guest_str == "/dev"
            {
                anyhow::bail!("sandbox mounts[{i}].guest_dir is a protected path: {guest_str}");
            }
            if mount.guest_dir.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }) {
                anyhow::bail!(
                    "sandbox mounts[{i}].guest_dir must not contain '.' or '..': {guest_str}"
                );
            }

            let mode = match mount.mode {
                WorkspaceMount::Ro => "ro",
                WorkspaceMount::Rw => "rw",
                WorkspaceMount::None => {
                    anyhow::bail!("sandbox mounts[{i}].mode must be \"ro\" or \"rw\"")
                },
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

        if self.config.no_network {
            args.push("--network=none".to_string());
        }

        if let Some(ref tz) = self.config.timezone {
            args.extend(["-e".to_string(), format!("TZ={tz}")]);
        }

        args.extend([
            "-e".to_string(),
            format!("MOLTIS_DATA_DIR={SANDBOX_GUEST_DATA_DIR}"),
        ]);

        args.extend(self.resource_args());
        args.extend(self.data_mount_args(id)?);
        args.extend(self.external_mount_args()?);

        args.push(image.to_string());
        args.extend(["sleep".to_string(), "infinity".to_string()]);
        Ok(args)
    }
}

#[async_trait]
impl Sandbox for DockerSandbox {
    fn backend_name(&self) -> &'static str {
        "docker"
    }

    async fn ensure_ready(&self, id: &SandboxId, image_override: Option<&str>) -> Result<()> {
        let name = self.container_name(id);

        // Check if container already running.
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

                // Stale container (older mount/env contract). Recreate it.
                let _ = tokio::process::Command::new("docker")
                    .args(["rm", "-f", &name])
                    .output()
                    .await;
            } else {
                // Container exists but is not running — recreate for a clean contract.
                let _ = tokio::process::Command::new("docker")
                    .args(["rm", "-f", &name])
                    .output()
                    .await;
            }
        }

        // Start a new container.
        let image = image_override.unwrap_or_else(|| self.image());
        let args = self.docker_run_args(&name, image, id)?;

        let output = tokio::process::Command::new("docker")
            .args(&args)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker run failed: {}", stderr.trim());
        }

        // Skip provisioning if the image is a pre-built instance sandbox image
        // (packages are already baked in — including /home/sandbox from the Dockerfile).
        let is_prebuilt = image.starts_with(&format!("{}:", self.image_repo()));
        if !is_prebuilt {
            provision_packages("docker", &name, &self.config.packages).await?;
        }

        Ok(())
    }

    async fn build_image(
        &self,
        base: &str,
        packages: &[String],
    ) -> Result<Option<BuildImageResult>> {
        if packages.is_empty() {
            return Ok(None);
        }

        let tag = sandbox_image_tag(self.image_repo(), base, packages);

        // Check if image already exists.
        if sandbox_image_exists("docker", &tag).await {
            info!(
                tag,
                "pre-built sandbox image already exists, skipping build"
            );
            return Ok(Some(BuildImageResult { tag, built: false }));
        }

        // Generate Dockerfile in a temp dir.
        let tmp_dir =
            std::env::temp_dir().join(format!("moltis-sandbox-build-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_dir)?;

        let pkg_list = packages.join(" ");
        let dockerfile = format!(
            "FROM {base}\n\
RUN apt-get update -qq && apt-get install -y -qq {pkg_list}\n\
RUN curl -fsSL https://mise.jdx.dev/install.sh | sh \
    && echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> /etc/profile.d/mise.sh\n\
RUN mkdir -p /home/sandbox\n\
ENV HOME=/home/sandbox\n\
ENV PATH=/home/sandbox/.local/bin:/root/.local/bin:$PATH\n\
WORKDIR /home/sandbox\n"
        );
        let dockerfile_path = tmp_dir.join("Dockerfile");
        std::fs::write(&dockerfile_path, &dockerfile)?;

        info!(tag, packages = %pkg_list, "building pre-built sandbox image");

        let output = tokio::process::Command::new("docker")
            .args(["build", "-t", &tag, "-f"])
            .arg(&dockerfile_path)
            .arg(&tmp_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await;

        // Clean up temp dir regardless of result.
        let _ = std::fs::remove_dir_all(&tmp_dir);

        let output = output?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker build failed for {tag}: {}", stderr.trim());
        }

        info!(tag, "pre-built sandbox image ready");
        Ok(Some(BuildImageResult { tag, built: true }))
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
            args.extend(["-e".to_string(), format!("{}={}", k, v)]);
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
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();

                if stdout.len() > opts.max_output_bytes {
                    stdout.truncate(opts.max_output_bytes);
                    stdout.push_str("\n... [output truncated]");
                }
                if stderr.len() > opts.max_output_bytes {
                    stderr.truncate(opts.max_output_bytes);
                    stderr.push_str("\n... [output truncated]");
                }

                Ok(ExecResult {
                    stdout,
                    stderr,
                    exit_code: output.status.code().unwrap_or(-1),
                })
            },
            Ok(Err(e)) => anyhow::bail!("docker exec failed: {e}"),
            Err(_) => anyhow::bail!("docker exec timed out after {}s", opts.timeout.as_secs()),
        }
    }

    async fn cleanup(&self, id: &SandboxId) -> Result<()> {
        let name = self.container_name(id);
        let _ = tokio::process::Command::new("docker")
            .args(["rm", "-f", &name])
            .output()
            .await;
        Ok(())
    }
}

/// No-op sandbox that passes through to direct execution.
pub struct NoSandbox;

#[async_trait]
impl Sandbox for NoSandbox {
    fn backend_name(&self) -> &'static str {
        "none"
    }

    async fn ensure_ready(&self, _id: &SandboxId, _image_override: Option<&str>) -> Result<()> {
        Ok(())
    }

    async fn exec(&self, _id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult> {
        crate::exec::exec_command(command, opts).await
    }

    async fn cleanup(&self, _id: &SandboxId) -> Result<()> {
        Ok(())
    }
}

/// Explicitly unsupported sandbox backend (used for `backend=apple-container`).
pub struct UnsupportedAppleContainerSandbox;

#[async_trait]
impl Sandbox for UnsupportedAppleContainerSandbox {
    fn backend_name(&self) -> &'static str {
        "apple-container"
    }

    async fn ensure_ready(&self, _id: &SandboxId, _image_override: Option<&str>) -> Result<()> {
        anyhow::bail!(
            "SANDBOX_BACKEND_UNSUPPORTED: backend=apple-container is not supported; set tools.exec.sandbox.backend=docker"
        );
    }

    async fn exec(&self, _id: &SandboxId, _command: &str, _opts: &ExecOpts) -> Result<ExecResult> {
        anyhow::bail!(
            "SANDBOX_BACKEND_UNSUPPORTED: backend=apple-container is not supported; set tools.exec.sandbox.backend=docker"
        );
    }

    async fn cleanup(&self, _id: &SandboxId) -> Result<()> {
        Ok(())
    }
}

/// Cgroup v2 sandbox using `systemd-run --user --scope` (Linux only, no root required).
#[cfg(target_os = "linux")]
pub struct CgroupSandbox {
    pub config: SandboxConfig,
}

#[cfg(target_os = "linux")]
impl CgroupSandbox {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    fn scope_name(&self, id: &SandboxId) -> String {
        let prefix = self
            .config
            .container_prefix
            .as_deref()
            .unwrap_or("moltis-sandbox");
        format!("{}-{}", prefix, id.key)
    }

    fn property_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let limits = &self.config.resource_limits;
        if let Some(ref mem) = limits.memory_limit {
            args.extend(["--property".to_string(), format!("MemoryMax={mem}")]);
        }
        if let Some(cpu) = limits.cpu_quota {
            let pct = (cpu * 100.0) as u64;
            args.extend(["--property".to_string(), format!("CPUQuota={pct}%")]);
        }
        if let Some(pids) = limits.pids_max {
            args.extend(["--property".to_string(), format!("TasksMax={pids}")]);
        }
        args
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl Sandbox for CgroupSandbox {
    fn backend_name(&self) -> &'static str {
        "cgroup"
    }

    async fn ensure_ready(&self, _id: &SandboxId, _image_override: Option<&str>) -> Result<()> {
        if !self.config.mounts.is_empty() {
            anyhow::bail!("external sandbox mounts are only supported on the Docker backend");
        }
        let output = tokio::process::Command::new("systemd-run")
            .arg("--version")
            .output()
            .await;
        match output {
            Ok(o) if o.status.success() => {
                debug!("systemd-run available");
                Ok(())
            },
            _ => anyhow::bail!("systemd-run not found; cgroup sandbox requires systemd"),
        }
    }

    async fn exec(&self, id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult> {
        let scope = self.scope_name(id);

        let mut args = vec![
            "--user".to_string(),
            "--scope".to_string(),
            "--unit".to_string(),
            scope,
        ];
        args.extend(self.property_args());
        args.extend(["sh".to_string(), "-c".to_string(), command.to_string()]);

        let mut cmd = tokio::process::Command::new("systemd-run");
        cmd.args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null());

        if let Some(ref dir) = opts.working_dir {
            cmd.current_dir(dir);
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }

        let child = cmd.spawn()?;
        let result = tokio::time::timeout(opts.timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();

                if stdout.len() > opts.max_output_bytes {
                    stdout.truncate(opts.max_output_bytes);
                    stdout.push_str("\n... [output truncated]");
                }
                if stderr.len() > opts.max_output_bytes {
                    stderr.truncate(opts.max_output_bytes);
                    stderr.push_str("\n... [output truncated]");
                }

                Ok(ExecResult {
                    stdout,
                    stderr,
                    exit_code: output.status.code().unwrap_or(-1),
                })
            },
            Ok(Err(e)) => anyhow::bail!("systemd-run exec failed: {e}"),
            Err(_) => anyhow::bail!(
                "systemd-run exec timed out after {}s",
                opts.timeout.as_secs()
            ),
        }
    }

    async fn cleanup(&self, id: &SandboxId) -> Result<()> {
        let scope = self.scope_name(id);
        let _ = tokio::process::Command::new("systemctl")
            .args(["--user", "stop", &format!("{scope}.scope")])
            .output()
            .await;
        Ok(())
    }
}

/// Apple Container sandbox using the `container` CLI (macOS 26+, Apple Silicon).
#[cfg(target_os = "macos")]
pub struct AppleContainerSandbox {
    pub config: SandboxConfig,
    name_generations: tokio::sync::RwLock<HashMap<String, u32>>,
}

#[cfg(target_os = "macos")]
impl AppleContainerSandbox {
    pub fn new(config: SandboxConfig) -> Self {
        Self {
            config,
            name_generations: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    fn image(&self) -> &str {
        self.config
            .image
            .as_deref()
            .unwrap_or(DEFAULT_SANDBOX_IMAGE)
    }

    fn container_prefix(&self) -> &str {
        self.config
            .container_prefix
            .as_deref()
            .unwrap_or("moltis-sandbox")
    }

    fn base_container_name(&self, id: &SandboxId) -> String {
        format!("{}-{}", self.container_prefix(), id.key)
    }

    async fn container_name(&self, id: &SandboxId) -> String {
        let base = self.base_container_name(id);
        let generation = self
            .name_generations
            .read()
            .await
            .get(&id.key)
            .copied()
            .unwrap_or(0);
        if generation == 0 {
            base
        } else {
            format!("{base}-g{generation}")
        }
    }

    async fn bump_container_generation(&self, id: &SandboxId) -> String {
        let next_generation = {
            let mut generations = self.name_generations.write().await;
            let entry = generations.entry(id.key.clone()).or_insert(0);
            *entry += 1;
            *entry
        };
        let base = self.base_container_name(id);
        let next_name = format!("{base}-g{next_generation}");
        warn!(
            session_key = %id.key,
            generation = next_generation,
            name = %next_name,
            "rotating apple container name generation after stale container conflict"
        );
        next_name
    }

    fn image_repo(&self) -> &str {
        self.container_prefix()
    }

    /// Check whether the `container` CLI is available.
    pub async fn is_available() -> bool {
        tokio::process::Command::new("container")
            .arg("--version")
            .output()
            .await
            .is_ok_and(|o| o.status.success())
    }

    async fn container_exists(name: &str) -> Result<bool> {
        let output = tokio::process::Command::new("container")
            .args(["inspect", name])
            .output()
            .await?;
        if !output.status.success() {
            return Ok(false);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(!(stdout.trim().is_empty() || stdout.trim() == "[]"))
    }

    async fn remove_container_force(name: &str) {
        let remove = tokio::process::Command::new("container")
            .args(["rm", "-f", name])
            .output()
            .await;

        match remove {
            Ok(output) if output.status.success() => {
                info!(name, "removed stale apple container");
            },
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                debug!(name, %stderr, "failed to remove stale apple container");
            },
            Err(e) => {
                debug!(name, error = %e, "failed to run apple container remove command");
            },
        }
    }

    async fn wait_for_container_absent(name: &str) {
        const MAX_WAIT_ITERS: usize = 20;
        const WAIT_MS: u64 = 100;

        for _ in 0..MAX_WAIT_ITERS {
            match Self::container_exists(name).await {
                Ok(false) => return,
                Ok(true) => tokio::time::sleep(std::time::Duration::from_millis(WAIT_MS)).await,
                Err(e) => {
                    debug!(name, error = %e, "failed while waiting for container removal");
                    return;
                },
            }
        }
    }

    async fn force_remove_and_wait(name: &str) {
        Self::remove_container_force(name).await;
        Self::wait_for_container_absent(name).await;
    }
}

/// Check whether the Apple Container system service is running.
#[cfg(target_os = "macos")]
fn is_apple_container_service_running() -> bool {
    std::process::Command::new("container")
        .args(["system", "status"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Try to start the Apple Container system service.
/// Returns `true` if the service was successfully started.
#[cfg(target_os = "macos")]
fn try_start_apple_container_service() -> bool {
    tracing::info!("apple container service is not running, starting it automatically");
    let result = std::process::Command::new("container")
        .args(["system", "start"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status();
    match result {
        Ok(status) if status.success() => {
            tracing::info!("apple container service started successfully");
            true
        },
        Ok(status) => {
            tracing::warn!(
                exit_code = status.code(),
                "failed to start apple container service; run `container system start` manually"
            );
            false
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to start apple container service; run `container system start` manually"
            );
            false
        },
    }
}

/// Ensure the Apple Container system service is running, starting it if needed.
/// Returns `true` if the service is running (either already or after starting).
#[cfg(target_os = "macos")]
fn ensure_apple_container_service() -> bool {
    if is_apple_container_service_running() {
        return true;
    }
    try_start_apple_container_service()
}

#[cfg(target_os = "macos")]
fn is_apple_container_service_error(stderr: &str) -> bool {
    stderr.contains("XPC connection error") || stderr.contains("Connection invalid")
}

#[cfg(target_os = "macos")]
fn is_apple_container_exists_error(stderr: &str) -> bool {
    stderr.contains("already exists") || stderr.contains("exists: \"container with id")
}

#[cfg(target_os = "macos")]
#[async_trait]
impl Sandbox for AppleContainerSandbox {
    fn backend_name(&self) -> &'static str {
        "apple-container"
    }

    async fn ensure_ready(&self, id: &SandboxId, image_override: Option<&str>) -> Result<()> {
        if !self.config.mounts.is_empty() {
            anyhow::bail!(
                "external sandbox mounts are not supported on the apple-container backend"
            );
        }
        let mut name = self.container_name(id).await;
        let image = image_override.unwrap_or_else(|| self.image());

        // Check if container exists and parse its state.
        // Note: `container inspect` returns exit 0 with empty `[]` for nonexistent
        // containers, so we must also check the output content.
        let check = tokio::process::Command::new("container")
            .args(["inspect", &name])
            .output()
            .await;

        if let Ok(output) = check
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);

            // Empty array means container doesn't exist — fall through to create.
            if stdout.trim() == "[]" || stdout.trim().is_empty() {
                info!(
                    name,
                    "apple container not found (inspect returned empty), creating"
                );
            } else if stdout.contains("\"running\"") {
                info!(name, "apple container already running");
                return Ok(());
            } else if stdout.contains("stopped") || stdout.contains("exited") {
                info!(name, "apple container stopped, restarting");
                let start = tokio::process::Command::new("container")
                    .args(["start", &name])
                    .output()
                    .await?;
                if !start.status.success() {
                    let stderr = String::from_utf8_lossy(&start.stderr);
                    warn!(name, %stderr, "container start failed, removing and recreating");
                    Self::force_remove_and_wait(&name).await;
                } else {
                    info!(name, "apple container restarted");
                    return Ok(());
                }
            } else {
                // Unknown state — log and recreate.
                info!(name, state = %stdout.chars().take(200).collect::<String>(), "apple container in unknown state, removing and recreating");
                Self::force_remove_and_wait(&name).await;
            }
        } else {
            info!(name, "apple container not found, creating");
        }

        // Container doesn't exist — create it.
        // Must pass `sleep infinity` so the container stays alive for subsequent
        // exec calls (the default entrypoint /bin/bash exits immediately without a TTY).
        info!(name, image, "creating apple container");
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            name.clone(),
        ];

        if let Some(ref tz) = self.config.timezone {
            args.extend(["-e".to_string(), format!("TZ={tz}")]);
        }

        args.extend([
            image.to_string(),
            "sleep".to_string(),
            "infinity".to_string(),
        ]);

        let mut run_args = args;
        let mut output = tokio::process::Command::new("container")
            .args(&run_args)
            .output()
            .await?;

        // Recovery loop for poisoned container names:
        // - If container metadata says "exists" but cleanup can't remove it, rotate
        //   to a new generation-specific name and retry.
        // - Also rotate on other non-service create failures to avoid repeatedly
        //   binding to a potentially corrupted name entry.
        for attempt in 0..2 {
            if output.status.success() {
                break;
            }

            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if is_apple_container_service_error(&stderr) {
                break;
            }

            if is_apple_container_exists_error(&stderr) {
                warn!(
                    name,
                    %stderr,
                    attempt,
                    "container already exists during create, removing stale entry and rotating name"
                );
                Self::force_remove_and_wait(&name).await;
            } else {
                warn!(
                    name,
                    %stderr,
                    attempt,
                    "container create failed, rotating name and retrying"
                );
            }

            name = self.bump_container_generation(id).await;
            if let Some(slot) = run_args
                .iter()
                .position(|arg| arg == "--name")
                .and_then(|idx| run_args.get_mut(idx + 1))
            {
                *slot = name.clone();
            }

            output = tokio::process::Command::new("container")
                .args(&run_args)
                .output()
                .await?;
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if is_apple_container_service_error(&stderr) {
                anyhow::bail!(
                    "apple container service is not running. \
                     Start it with `container system start` and restart moltis"
                );
            }
            anyhow::bail!(
                "container run failed for {name} (image={image}): {}",
                stderr.trim()
            );
        }

        info!(name, image, "apple container created and running");

        // Skip provisioning if the image is a pre-built instance sandbox image
        // (packages are already baked in — including /home/sandbox from the Dockerfile).
        let is_prebuilt = image.starts_with(&format!("{}:", self.image_repo()));
        if !is_prebuilt {
            provision_packages("container", &name, &self.config.packages).await?;
        }

        Ok(())
    }

    async fn exec(&self, id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult> {
        let name = self.container_name(id).await;
        info!(name, command, "apple container exec");

        let mut args = vec!["exec".to_string(), name.clone()];

        // Apple Container CLI doesn't support -e flags, so prepend export
        // statements to inject env vars into the shell.
        let mut prefix = String::new();
        for (k, v) in &opts.env {
            // Shell-escape the value with single quotes.
            let escaped = v.replace('\'', "'\\''");
            prefix.push_str(&format!("export {k}='{escaped}'; "));
        }

        let full_command = if let Some(ref dir) = opts.working_dir {
            format!("{prefix}cd {} && {command}", dir.display())
        } else {
            format!("{prefix}{command}")
        };

        args.extend(["sh".to_string(), "-c".to_string(), full_command]);

        let child = tokio::process::Command::new("container")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .spawn()?;

        let result = tokio::time::timeout(opts.timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();

                if stdout.len() > opts.max_output_bytes {
                    stdout.truncate(opts.max_output_bytes);
                    stdout.push_str("\n... [output truncated]");
                }
                if stderr.len() > opts.max_output_bytes {
                    stderr.truncate(opts.max_output_bytes);
                    stderr.push_str("\n... [output truncated]");
                }

                debug!(
                    name,
                    exit_code,
                    stdout_len = stdout.len(),
                    stderr_len = stderr.len(),
                    "apple container exec complete"
                );
                Ok(ExecResult {
                    stdout,
                    stderr,
                    exit_code,
                })
            },
            Ok(Err(e)) => {
                warn!(name, %e, "apple container exec spawn failed");
                anyhow::bail!("container exec failed for {name}: {e}")
            },
            Err(_) => {
                warn!(
                    name,
                    timeout_secs = opts.timeout.as_secs(),
                    "apple container exec timed out"
                );
                anyhow::bail!(
                    "container exec timed out for {name} after {}s",
                    opts.timeout.as_secs()
                )
            },
        }
    }

    async fn build_image(
        &self,
        base: &str,
        packages: &[String],
    ) -> Result<Option<BuildImageResult>> {
        if packages.is_empty() {
            return Ok(None);
        }

        let tag = sandbox_image_tag(self.image_repo(), base, packages);

        if sandbox_image_exists("container", &tag).await {
            info!(
                tag,
                "pre-built sandbox image already exists, skipping build"
            );
            return Ok(Some(BuildImageResult { tag, built: false }));
        }

        let tmp_dir =
            std::env::temp_dir().join(format!("moltis-sandbox-build-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_dir)?;

        let pkg_list = packages.join(" ");
        let dockerfile = format!(
            "FROM {base}\n\
RUN apt-get update -qq && apt-get install -y -qq {pkg_list}\n\
RUN curl -fsSL https://mise.jdx.dev/install.sh | sh \
    && echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> /etc/profile.d/mise.sh\n\
RUN mkdir -p /home/sandbox\n\
ENV HOME=/home/sandbox\n\
ENV PATH=/home/sandbox/.local/bin:/root/.local/bin:$PATH\n\
WORKDIR /home/sandbox\n"
        );
        let dockerfile_path = tmp_dir.join("Dockerfile");
        std::fs::write(&dockerfile_path, &dockerfile)?;

        info!(tag, packages = %pkg_list, "building pre-built sandbox image (apple container)");

        let output = tokio::process::Command::new("container")
            .args(["build", "-t", &tag, "-f"])
            .arg(&dockerfile_path)
            .arg(&tmp_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await;

        let _ = std::fs::remove_dir_all(&tmp_dir);

        let output = output?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("XPC connection error") || stderr.contains("Connection invalid") {
                anyhow::bail!(
                    "apple container service is not running. \
                     Start it with `container system start` and restart moltis"
                );
            }
            anyhow::bail!("container build failed for {tag}: {}", stderr.trim());
        }

        info!(tag, "pre-built sandbox image ready (apple container)");
        Ok(Some(BuildImageResult { tag, built: true }))
    }

    async fn cleanup(&self, id: &SandboxId) -> Result<()> {
        let base = self.base_container_name(id);
        let max_generation = self
            .name_generations
            .read()
            .await
            .get(&id.key)
            .copied()
            .unwrap_or(0);

        for generation in 0..=max_generation {
            let name = if generation == 0 {
                base.clone()
            } else {
                format!("{base}-g{generation}")
            };
            info!(name, "cleaning up apple container");
            let _ = tokio::process::Command::new("container")
                .args(["stop", &name])
                .output()
                .await;
            let _ = tokio::process::Command::new("container")
                .args(["rm", &name])
                .output()
                .await;
        }
        self.name_generations.write().await.remove(&id.key);
        Ok(())
    }
}

/// Create the appropriate sandbox backend based on config and platform.
pub fn create_sandbox(config: SandboxConfig) -> Arc<dyn Sandbox> {
    if config.mode == SandboxMode::Off {
        return Arc::new(NoSandbox);
    }

    select_backend(config)
}

/// Create a real sandbox backend regardless of mode (for use by SandboxRouter,
/// which may need a real backend even when global mode is Off because per-session
/// overrides can enable sandboxing dynamically).
fn create_sandbox_backend(config: SandboxConfig) -> Arc<dyn Sandbox> {
    select_backend(config)
}

/// Select the sandbox backend based on config and platform availability.
///
/// When `backend` is `"auto"` (the default):
/// - Use Docker when available.
/// - Fall back to direct execution otherwise.
fn select_backend(config: SandboxConfig) -> Arc<dyn Sandbox> {
    match config.backend.as_str() {
        "docker" => Arc::new(DockerSandbox::new(config)),
        "apple-container" => Arc::new(UnsupportedAppleContainerSandbox),
        _ => auto_detect_backend(config),
    }
}

fn auto_detect_backend(config: SandboxConfig) -> Arc<dyn Sandbox> {
    if should_use_docker_backend(is_cli_available("docker"), is_docker_daemon_available()) {
        tracing::info!("sandbox backend: docker");
        return Arc::new(DockerSandbox::new(config));
    }

    if is_cli_available("docker") {
        tracing::warn!(
            "docker CLI detected but daemon is not accessible; sandboxed execution will use direct host access"
        );
    }

    tracing::warn!(
        "no usable container runtime found; sandboxed execution will use direct host access"
    );
    Arc::new(NoSandbox)
}

fn should_use_docker_backend(docker_cli_available: bool, docker_daemon_available: bool) -> bool {
    docker_cli_available && docker_daemon_available
}

fn is_docker_daemon_available() -> bool {
    std::process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check whether a CLI tool is available on PATH.
fn is_cli_available(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Events emitted by the sandbox subsystem for UI feedback.
#[derive(Debug, Clone)]
pub enum SandboxEvent {
    /// Package provisioning started (Apple Container per-container install).
    Provisioning {
        container: String,
        packages: Vec<String>,
    },
    /// Package provisioning finished.
    Provisioned { container: String },
    /// Package provisioning failed (non-fatal).
    ProvisionFailed { container: String, error: String },
}

fn sanitize_sandbox_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Parse a channel session key into `(channel_type, account_id, chat_id)` if possible.
///
/// V1: best-effort, relies on `channel:<account_id>:<chat_id...>` shape.
fn parse_channel_session_key(session_key: &str) -> Option<(String, String, String)> {
    let mut it = session_key.split(':');
    let channel = it.next()?.to_string();
    let account = it.next()?.to_string();
    let rest: Vec<&str> = it.collect();
    if rest.is_empty() {
        return None;
    }
    Some((channel, account, rest.join(":")))
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

/// Routes sandbox decisions per-session, with per-session overrides on top of global config.
pub struct SandboxRouter {
    config: SandboxConfig,
    backend: Arc<dyn Sandbox>,
    /// Serialize `ensure_ready` per effective sandbox key to avoid concurrent
    /// container creation races when multiple sessions share the same sandbox
    /// (e.g. multiple bots in one Telegram group with `scope=chat`).
    ensure_ready_locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Per-session overrides: true = sandboxed, false = direct execution.
    overrides: RwLock<HashMap<String, bool>>,
    /// Per-session image overrides.
    image_overrides: RwLock<HashMap<String, String>>,
    /// Runtime override for the global default image (set via API, persisted externally).
    global_image_override: RwLock<Option<String>>,
    /// Last-used timestamps keyed by effective sandbox key.
    last_used_ms: std::sync::Mutex<HashMap<String, u64>>,
    /// In-process lease counts keyed by effective sandbox key (best-effort).
    lease_counts: std::sync::Arc<std::sync::Mutex<HashMap<String, u32>>>,
    /// Event channel for sandbox events (provision start/done/error).
    event_tx: tokio::sync::broadcast::Sender<SandboxEvent>,
}

impl SandboxRouter {
    pub fn new(config: SandboxConfig) -> Self {
        // Always create a real sandbox backend, even when global mode is Off,
        // because per-session overrides can enable sandboxing dynamically.
        let backend = create_sandbox_backend(config.clone());
        let (event_tx, _) = tokio::sync::broadcast::channel(32);
        Self {
            config,
            backend,
            ensure_ready_locks: tokio::sync::Mutex::new(HashMap::new()),
            overrides: RwLock::new(HashMap::new()),
            image_overrides: RwLock::new(HashMap::new()),
            global_image_override: RwLock::new(None),
            last_used_ms: std::sync::Mutex::new(HashMap::new()),
            lease_counts: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
            event_tx,
        }
    }

    /// Create a router with a custom sandbox backend (useful for testing).
    pub fn with_backend(config: SandboxConfig, backend: Arc<dyn Sandbox>) -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(32);
        Self {
            config,
            backend,
            ensure_ready_locks: tokio::sync::Mutex::new(HashMap::new()),
            overrides: RwLock::new(HashMap::new()),
            image_overrides: RwLock::new(HashMap::new()),
            global_image_override: RwLock::new(None),
            last_used_ms: std::sync::Mutex::new(HashMap::new()),
            lease_counts: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
            event_tx,
        }
    }

    /// Subscribe to sandbox events (provision start/done/error).
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<SandboxEvent> {
        self.event_tx.subscribe()
    }

    /// Emit a sandbox event. Silently drops if no subscribers.
    pub fn emit_event(&self, event: SandboxEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Check whether a session should run sandboxed.
    /// Per-session override takes priority, then falls back to global mode.
    pub async fn is_sandboxed(&self, session_key: &str) -> bool {
        if let Some(&override_val) = self.overrides.read().await.get(session_key) {
            return override_val;
        }
        match self.config.mode {
            SandboxMode::Off => false,
            SandboxMode::All => true,
            SandboxMode::NonMain => session_key != "main",
        }
    }

    /// Set a per-session sandbox override.
    pub async fn set_override(&self, session_key: &str, enabled: bool) {
        self.overrides
            .write()
            .await
            .insert(session_key.to_string(), enabled);
    }

    /// Remove a per-session override (revert to global mode).
    pub async fn remove_override(&self, session_key: &str) {
        self.overrides.write().await.remove(session_key);
    }

    /// Derive a SandboxId for a given session key.
    /// The key is sanitized for use as a container name (only alphanumeric, dash, underscore, dot).
    pub fn sandbox_id_for(&self, session_key: &str) -> SandboxId {
        let effective_key = self.effective_sandbox_key(session_key);
        let sanitized = sanitize_sandbox_key(&effective_key);
        SandboxId {
            scope: self.config.scope.clone(),
            key: sanitized,
        }
    }

    /// Ensure the sandbox container for a session is ready.
    ///
    /// Serialized per effective sandbox key to avoid races when multiple
    /// sessions map to the same container (shared scopes like `chat|bot|global`).
    pub async fn ensure_ready_for_session(
        &self,
        session_key: &str,
        image_override: Option<&str>,
    ) -> Result<SandboxId> {
        let effective_key = self.effective_sandbox_key(session_key);
        let lock = {
            let mut locks = self.ensure_ready_locks.lock().await;
            locks
                .entry(effective_key)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let id = self.sandbox_id_for(session_key);
        self.backend.ensure_ready(&id, image_override).await?;
        Ok(id)
    }

    /// Compute the effective (unsanitized) sandbox key for a session.
    ///
    /// This is the canonical identifier for sandbox reuse boundaries.
    pub fn effective_sandbox_key(&self, session_key: &str) -> String {
        match self.config.scope {
            SandboxScope::Session => session_key.to_string(),
            SandboxScope::Global => "global".to_string(),
            SandboxScope::Bot => {
                if let Some((channel, account, _chat)) = parse_channel_session_key(session_key) {
                    format!("{channel}:bot:{account}")
                } else {
                    session_key.to_string()
                }
            },
            SandboxScope::Chat => {
                if let Some((channel, _account, chat)) = parse_channel_session_key(session_key) {
                    // V1: Keep DM behavior stable — do not apply chat-scope to Telegram DMs.
                    if channel == "telegram" {
                        let chat_id = chat.split(':').next().unwrap_or("");
                        if !chat_id.starts_with('-') {
                            return session_key.to_string();
                        }
                    }
                    format!("{channel}:chat:{chat}")
                } else {
                    session_key.to_string()
                }
            },
        }
    }

    /// Acquire an in-process lease for an effective sandbox key.
    ///
    /// Used to prevent TTL pruning from removing a sandbox while it is actively in use.
    pub fn acquire_lease(&self, session_key: &str) -> SandboxLease {
        let key = self.effective_sandbox_key(session_key);
        {
            let mut leases = self.lease_counts.lock().unwrap_or_else(|e| e.into_inner());
            *leases.entry(key.clone()).or_insert(0) += 1;
        }
        SandboxLease {
            key,
            lease_counts: std::sync::Arc::clone(&self.lease_counts),
        }
    }

    /// Record that a sandbox key was used (updates last_used timestamp).
    pub fn touch(&self, session_key: &str) {
        self.touch_effective_key(&self.effective_sandbox_key(session_key));
    }

    pub fn touch_effective_key(&self, effective_key: &str) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut last = self.last_used_ms.lock().unwrap_or_else(|e| e.into_inner());
        last.insert(effective_key.to_string(), now_ms);
    }

    /// Prune idle sandboxes based on `idle_ttl_secs` (best-effort).
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
            if let Err(e) = self.cleanup_effective_key(&effective_key).await {
                tracing::warn!(effective_key, error = %e, "sandbox prune failed");
            }
        }
    }

    /// Clean up sandbox resources for an effective sandbox key.
    pub async fn cleanup_effective_key(&self, effective_key: &str) -> Result<()> {
        let id = SandboxId {
            scope: self.config.scope.clone(),
            key: sanitize_sandbox_key(effective_key),
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

    /// Clean up sandbox resources for a session.
    pub async fn cleanup_session(&self, session_key: &str) -> Result<()> {
        let id = self.sandbox_id_for(session_key);
        self.backend.cleanup(&id).await?;
        self.remove_override(session_key).await;
        self.remove_image_override(session_key).await;
        {
            let effective_key = self.effective_sandbox_key(session_key);
            let mut locks = self.ensure_ready_locks.lock().await;
            locks.remove(&effective_key);
        }
        Ok(())
    }

    /// Clean up per-session override state (without touching containers).
    pub async fn cleanup_session_state(&self, session_key: &str) {
        self.remove_override(session_key).await;
        self.image_overrides.write().await.remove(session_key);
    }

    /// Access the sandbox backend.
    pub fn backend(&self) -> &Arc<dyn Sandbox> {
        &self.backend
    }

    /// Access the global sandbox mode.
    pub fn mode(&self) -> &SandboxMode {
        &self.config.mode
    }

    /// Access the global sandbox config.
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Human-readable name of the sandbox backend (e.g. "docker", "apple-container").
    pub fn backend_name(&self) -> &'static str {
        self.backend.backend_name()
    }

    /// Set a per-session image override.
    pub async fn set_image_override(&self, session_key: &str, image: String) {
        if self.config.scope != SandboxScope::Session {
            // Shared scopes must not allow per-session image overrides.
            return;
        }
        self.image_overrides
            .write()
            .await
            .insert(session_key.to_string(), image);
    }

    /// Remove a per-session image override.
    pub async fn remove_image_override(&self, session_key: &str) {
        if self.config.scope != SandboxScope::Session {
            return;
        }
        self.image_overrides.write().await.remove(session_key);
    }

    /// Set a runtime override for the global default image.
    /// Pass `None` to revert to the config/hardcoded default.
    pub async fn set_global_image(&self, image: Option<String>) {
        *self.global_image_override.write().await = image;
    }

    /// Get the current effective default image (runtime override > config > hardcoded).
    pub async fn default_image(&self) -> String {
        if let Some(ref img) = *self.global_image_override.read().await {
            return img.clone();
        }
        self.config
            .image
            .clone()
            .unwrap_or_else(|| DEFAULT_SANDBOX_IMAGE.to_string())
    }

    /// Resolve the container image for a session.
    ///
    /// Priority (highest to lowest):
    /// 1. `skill_image` — from a skill's Dockerfile cache
    /// 2. Per-session override (`session.sandbox_image`)
    /// 3. Runtime global override (`set_global_image`)
    /// 4. Global config (`config.tools.exec.sandbox.image`)
    /// 5. Default constant (`DEFAULT_SANDBOX_IMAGE`)
    pub async fn resolve_image(&self, session_key: &str, skill_image: Option<&str>) -> String {
        if let Some(img) = skill_image {
            return img.to_string();
        }
        if self.config.scope == SandboxScope::Session {
            if let Some(img) = self.image_overrides.read().await.get(session_key) {
                return img.clone();
            }
        }
        self.default_image().await
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct TestSandbox {
        name: &'static str,
        ensure_ready_error: Option<String>,
        exec_error: Option<String>,
        ensure_ready_calls: AtomicUsize,
        exec_calls: AtomicUsize,
        cleanup_calls: AtomicUsize,
    }

    impl TestSandbox {
        fn new(
            name: &'static str,
            ensure_ready_error: Option<&str>,
            exec_error: Option<&str>,
        ) -> Self {
            Self {
                name,
                ensure_ready_error: ensure_ready_error.map(ToOwned::to_owned),
                exec_error: exec_error.map(ToOwned::to_owned),
                ensure_ready_calls: AtomicUsize::new(0),
                exec_calls: AtomicUsize::new(0),
                cleanup_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl Sandbox for TestSandbox {
        fn backend_name(&self) -> &'static str {
            self.name
        }

        async fn ensure_ready(&self, _id: &SandboxId, _image_override: Option<&str>) -> Result<()> {
            self.ensure_ready_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(ref error) = self.ensure_ready_error {
                anyhow::bail!("{error}");
            }
            Ok(())
        }

        async fn exec(
            &self,
            _id: &SandboxId,
            _command: &str,
            _opts: &ExecOpts,
        ) -> Result<ExecResult> {
            self.exec_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(ref error) = self.exec_error {
                anyhow::bail!("{error}");
            }
            Ok(ExecResult {
                stdout: "ok".into(),
                stderr: String::new(),
                exit_code: 0,
            })
        }

        async fn cleanup(&self, _id: &SandboxId) -> Result<()> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn test_sandbox_mode_display() {
        assert_eq!(SandboxMode::Off.to_string(), "off");
        assert_eq!(SandboxMode::NonMain.to_string(), "non-main");
        assert_eq!(SandboxMode::All.to_string(), "all");
    }

    #[test]
    fn test_sandbox_scope_display() {
        assert_eq!(SandboxScope::Session.to_string(), "session");
        assert_eq!(SandboxScope::Chat.to_string(), "chat");
        assert_eq!(SandboxScope::Bot.to_string(), "bot");
        assert_eq!(SandboxScope::Global.to_string(), "global");
    }

    #[test]
    fn test_workspace_mount_display() {
        assert_eq!(WorkspaceMount::None.to_string(), "none");
        assert_eq!(WorkspaceMount::Ro.to_string(), "ro");
        assert_eq!(WorkspaceMount::Rw.to_string(), "rw");
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert!(limits.memory_limit.is_none());
        assert!(limits.cpu_quota.is_none());
        assert!(limits.pids_max.is_none());
    }

    #[test]
    fn test_resource_limits_serde() {
        let json = r#"{"memory_limit":"512M","cpu_quota":1.5,"pids_max":100}"#;
        let limits: ResourceLimits = serde_json::from_str(json).unwrap();
        assert_eq!(limits.memory_limit.as_deref(), Some("512M"));
        assert_eq!(limits.cpu_quota, Some(1.5));
        assert_eq!(limits.pids_max, Some(100));
    }

    #[test]
    fn test_sandbox_config_serde() {
        let json = r#"{
            "mode": "all",
            "scope": "session",
            "data_mount": "rw",
            "data_mount_type": "bind",
            "data_mount_source": "/srv/moltis-data",
            "no_network": true,
            "resource_limits": {"memory_limit": "1G"}
        }"#;
        let config: SandboxConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, SandboxMode::All);
        assert_eq!(config.data_mount, WorkspaceMount::Rw);
        assert_eq!(config.data_mount_type, Some(DataMountType::Bind));
        assert_eq!(
            config.data_mount_source.as_deref(),
            Some("/srv/moltis-data")
        );
        assert!(config.no_network);
        assert_eq!(config.resource_limits.memory_limit.as_deref(), Some("1G"));
    }

    #[test]
    fn test_docker_resource_args() {
        let config = SandboxConfig {
            resource_limits: ResourceLimits {
                memory_limit: Some("256M".into()),
                cpu_quota: Some(0.5),
                pids_max: Some(50),
            },
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let args = docker.resource_args();
        assert_eq!(
            args,
            vec!["--memory", "256M", "--cpus", "0.5", "--pids-limit", "50"]
        );
    }

    #[test]
    fn test_docker_data_mount_args_bind_ro() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("USER.md"), "user\n").unwrap();
        std::fs::write(dir.path().join("PEOPLE.md"), "people\n").unwrap();
        let config = SandboxConfig {
            data_mount: WorkspaceMount::Ro,
            data_mount_type: Some(DataMountType::Bind),
            data_mount_source: Some(dir.path().display().to_string()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "main".into(),
        };
        let view = public_data_view_dir(&dir.path().display().to_string(), &id.key);
        let args = docker.data_mount_args(&id).unwrap();
        assert_eq!(
            args,
            vec![
                "-v".to_string(),
                format!("{}:/moltis/data:ro", view.display())
            ]
        );
    }

    #[test]
    fn test_docker_data_mount_args_volume_rw() {
        let config = SandboxConfig {
            data_mount: WorkspaceMount::Rw,
            data_mount_type: Some(DataMountType::Volume),
            data_mount_source: Some("moltis-data".into()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "main".into(),
        };
        let args = docker.data_mount_args(&id).unwrap();
        assert_eq!(
            args,
            vec!["-v".to_string(), "moltis-data:/moltis/data:rw".to_string()]
        );
    }

    #[test]
    fn test_docker_data_mount_args_missing_fails_fast() {
        let config = SandboxConfig {
            data_mount: WorkspaceMount::None,
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "main".into(),
        };
        let err = docker.data_mount_args(&id).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_DATA_MOUNT_REQUIRED"));
    }

    #[test]
    fn test_docker_data_mount_args_invalid_bind_source_fails_fast() {
        let config = SandboxConfig {
            data_mount: WorkspaceMount::Ro,
            data_mount_type: Some(DataMountType::Bind),
            data_mount_source: Some("relative/path".into()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "main".into(),
        };
        let err = docker.data_mount_args(&id).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_DATA_MOUNT_INVALID"));
    }

    #[test]
    fn test_docker_data_mount_args_invalid_bind_source_with_colon_fails_fast() {
        let config = SandboxConfig {
            data_mount: WorkspaceMount::Ro,
            data_mount_type: Some(DataMountType::Bind),
            data_mount_source: Some("/srv/moltis:data".into()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "main".into(),
        };
        let err = docker.data_mount_args(&id).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_DATA_MOUNT_INVALID"));
    }

    #[test]
    fn test_docker_data_mount_args_invalid_volume_source_fails_fast() {
        let config = SandboxConfig {
            data_mount: WorkspaceMount::Ro,
            data_mount_type: Some(DataMountType::Volume),
            data_mount_source: Some("bad/name".into()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "main".into(),
        };
        let err = docker.data_mount_args(&id).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_DATA_MOUNT_INVALID"));
    }

    #[test]
    fn test_docker_data_mount_args_invalid_volume_source_with_colon_fails_fast() {
        let config = SandboxConfig {
            data_mount: WorkspaceMount::Ro,
            data_mount_type: Some(DataMountType::Volume),
            data_mount_source: Some("bad:name".into()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "main".into(),
        };
        let err = docker.data_mount_args(&id).unwrap_err().to_string();
        assert!(err.contains("SANDBOX_DATA_MOUNT_INVALID"));
    }

    #[test]
    fn test_docker_run_args_includes_data_dir_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("USER.md"), "user\n").unwrap();
        std::fs::write(dir.path().join("PEOPLE.md"), "people\n").unwrap();
        let config = SandboxConfig {
            data_mount: WorkspaceMount::Ro,
            data_mount_type: Some(DataMountType::Bind),
            data_mount_source: Some(dir.path().display().to_string()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "test".into(),
        };
        let args = docker
            .docker_run_args("moltis-sandbox-test", DEFAULT_SANDBOX_IMAGE, &id)
            .unwrap();
        assert!(args.contains(&"-e".to_string()));
        assert!(args.contains(&format!("MOLTIS_DATA_DIR={SANDBOX_GUEST_DATA_DIR}")));
        assert!(
            args.iter()
                .any(|a| a.contains(&format!(".sandbox_views/{}:{SANDBOX_GUEST_DATA_DIR}:ro", id.key)))
        );
    }

    #[test]
    fn test_prepare_public_data_view_only_copies_public_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("USER.md"), "user-public\n").unwrap();
        std::fs::write(dir.path().join("PEOPLE.md"), "people-public\n").unwrap();
        std::fs::create_dir_all(dir.path().join("people/default")).unwrap();
        std::fs::write(
            dir.path().join("people/default/IDENTITY.md"),
            "---\nname: default\n---\nsecret\n",
        )
        .unwrap();

        let view_dir = prepare_public_data_view(&dir.path().display().to_string(), "main").unwrap();

        let mut names: Vec<String> = std::fs::read_dir(&view_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["PEOPLE.md".to_string(), "USER.md".to_string()]);

        let user = std::fs::read_to_string(view_dir.join("USER.md")).unwrap();
        let people = std::fs::read_to_string(view_dir.join("PEOPLE.md")).unwrap();
        assert_eq!(user, "user-public\n");
        assert_eq!(people, "people-public\n");
        assert!(!view_dir.join("people").exists());
    }

    #[test]
    fn test_docker_external_mount_args_ro() {
        let dir = tempfile::tempdir().unwrap();
        let allow_root = dir.path().join("allow");
        let host_dir = allow_root.join("proj");
        std::fs::create_dir_all(&host_dir).unwrap();

        let config = SandboxConfig {
            mounts: vec![SandboxMount {
                host_dir: host_dir.clone(),
                guest_dir: std::path::PathBuf::from("/mnt/host/proj"),
                mode: WorkspaceMount::Ro,
            }],
            mount_allowlist: vec![allow_root.clone()],
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let args = docker.external_mount_args().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-v");
        assert!(args[1].contains(":/mnt/host/proj:ro"));
    }

    #[cfg(unix)]
    #[test]
    fn test_docker_external_mount_args_symlink_escape_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let allow_root = dir.path().join("allow");
        let outside_root = dir.path().join("outside");
        std::fs::create_dir_all(&allow_root).unwrap();
        std::fs::create_dir_all(&outside_root).unwrap();

        let link = allow_root.join("link");
        symlink(&outside_root, &link).unwrap();

        let config = SandboxConfig {
            mounts: vec![SandboxMount {
                host_dir: link,
                guest_dir: std::path::PathBuf::from("/mnt/host/outside"),
                mode: WorkspaceMount::Ro,
            }],
            mount_allowlist: vec![allow_root],
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let err = docker.external_mount_args().unwrap_err().to_string();
        assert!(err.contains("outside mount_allowlist"));
    }

    #[test]
    fn test_create_sandbox_off() {
        let config = SandboxConfig {
            mode: SandboxMode::Off,
            ..Default::default()
        };
        let sandbox = create_sandbox(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "test".into(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            sandbox.ensure_ready(&id, None).await.unwrap();
            sandbox.cleanup(&id).await.unwrap();
        });
    }

    #[tokio::test]
    async fn test_no_sandbox_exec() {
        let sandbox = NoSandbox;
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "test".into(),
        };
        let opts = ExecOpts::default();
        let result = sandbox.exec(&id, "echo sandbox-test", &opts).await.unwrap();
        assert_eq!(result.stdout.trim(), "sandbox-test");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_docker_container_name() {
        let config = SandboxConfig {
            container_prefix: Some("my-prefix".into()),
            ..Default::default()
        };
        let docker = DockerSandbox::new(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "abc123".into(),
        };
        assert_eq!(docker.container_name(&id), "my-prefix-abc123");
    }

    #[tokio::test]
    async fn test_sandbox_router_default_all() {
        let config = SandboxConfig::default(); // mode = All
        let router = SandboxRouter::new(config);
        assert!(router.is_sandboxed("main").await);
        assert!(router.is_sandboxed("session:abc").await);
    }

    #[tokio::test]
    async fn test_sandbox_router_mode_off() {
        let config = SandboxConfig {
            mode: SandboxMode::Off,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert!(!router.is_sandboxed("main").await);
        assert!(!router.is_sandboxed("session:abc").await);
    }

    #[tokio::test]
    async fn test_sandbox_router_mode_all() {
        let config = SandboxConfig {
            mode: SandboxMode::All,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert!(router.is_sandboxed("main").await);
        assert!(router.is_sandboxed("session:abc").await);
    }

    #[tokio::test]
    async fn test_sandbox_router_mode_non_main() {
        let config = SandboxConfig {
            mode: SandboxMode::NonMain,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert!(!router.is_sandboxed("main").await);
        assert!(router.is_sandboxed("session:abc").await);
    }

    #[tokio::test]
    async fn test_sandbox_router_override() {
        let config = SandboxConfig {
            mode: SandboxMode::Off,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert!(!router.is_sandboxed("session:abc").await);

        router.set_override("session:abc", true).await;
        assert!(router.is_sandboxed("session:abc").await);

        router.set_override("session:abc", false).await;
        assert!(!router.is_sandboxed("session:abc").await);

        router.remove_override("session:abc").await;
        assert!(!router.is_sandboxed("session:abc").await);
    }

    #[tokio::test]
    async fn test_sandbox_router_override_overrides_mode() {
        let config = SandboxConfig {
            mode: SandboxMode::All,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert!(router.is_sandboxed("main").await);

        // Override to disable sandbox for main
        router.set_override("main", false).await;
        assert!(!router.is_sandboxed("main").await);
    }

    #[test]
    fn test_backend_name_docker() {
        let sandbox = DockerSandbox::new(SandboxConfig::default());
        assert_eq!(sandbox.backend_name(), "docker");
    }

    #[test]
    fn test_backend_name_none() {
        let sandbox = NoSandbox;
        assert_eq!(sandbox.backend_name(), "none");
    }

    #[test]
    fn test_sandbox_router_backend_name() {
        // With "auto", the backend depends on what's available on the host.
        let config = SandboxConfig::default();
        let router = SandboxRouter::new(config);
        let name = router.backend_name();
        assert!(
            name == "docker" || name == "apple-container" || name == "none",
            "unexpected backend: {name}"
        );
    }

    #[test]
    fn test_sandbox_router_explicit_docker_backend() {
        let config = SandboxConfig {
            backend: "docker".into(),
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert_eq!(router.backend_name(), "docker");
    }

    #[test]
    fn test_sandbox_router_config_accessor() {
        let config = SandboxConfig {
            mode: SandboxMode::NonMain,
            scope: SandboxScope::Bot,
            image: Some("alpine:latest".into()),
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert_eq!(*router.mode(), SandboxMode::NonMain);
        assert_eq!(router.config().scope, SandboxScope::Bot);
        assert_eq!(router.config().image.as_deref(), Some("alpine:latest"));
    }

    #[test]
    fn test_sandbox_router_sandbox_id_for() {
        let config = SandboxConfig {
            scope: SandboxScope::Session,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        let id = router.sandbox_id_for("session:abc");
        assert_eq!(id.key, "session-abc");
        // Plain alphanumeric keys pass through unchanged.
        let id2 = router.sandbox_id_for("main");
        assert_eq!(id2.key, "main");
    }

    struct ConcurrencyDetectSandbox {
        in_flight: std::sync::atomic::AtomicUsize,
        max_in_flight: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Sandbox for ConcurrencyDetectSandbox {
        fn backend_name(&self) -> &'static str {
            "detect"
        }

        async fn ensure_ready(&self, _id: &SandboxId, _image_override: Option<&str>) -> Result<()> {
            use std::sync::atomic::Ordering;
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            let mut prev = self.max_in_flight.load(Ordering::SeqCst);
            while now > prev {
                match self.max_in_flight.compare_exchange(
                    prev,
                    now,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => break,
                    Err(p) => prev = p,
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }

        async fn exec(
            &self,
            _id: &SandboxId,
            _command: &str,
            _opts: &ExecOpts,
        ) -> Result<ExecResult> {
            anyhow::bail!("not implemented for test")
        }

        async fn cleanup(&self, _id: &SandboxId) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_router_ensure_ready_serialized_for_shared_scope() {
        let detect = Arc::new(ConcurrencyDetectSandbox {
            in_flight: std::sync::atomic::AtomicUsize::new(0),
            max_in_flight: std::sync::atomic::AtomicUsize::new(0),
        });
        let backend: Arc<dyn Sandbox> = detect.clone();

        let config = SandboxConfig {
            scope: SandboxScope::Chat,
            ..Default::default()
        };
        let router = SandboxRouter::with_backend(config, backend);

        // Different sessions that map to the same effective key when scope=chat.
        let s1 = "telegram:bot1:-100";
        let s2 = "telegram:bot2:-100";

        let (r1, r2) = tokio::join!(
            router.ensure_ready_for_session(s1, Some("ubuntu:25.10")),
            router.ensure_ready_for_session(s2, Some("ubuntu:25.10")),
        );
        r1.unwrap();
        r2.unwrap();

        use std::sync::atomic::Ordering;
        assert_eq!(detect.max_in_flight.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_effective_sandbox_key_session_scope() {
        let config = SandboxConfig {
            scope: SandboxScope::Session,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert_eq!(
            router.effective_sandbox_key("telegram:lovely:-1"),
            "telegram:lovely:-1"
        );
    }

    #[test]
    fn test_effective_sandbox_key_chat_scope_telegram_group() {
        let config = SandboxConfig {
            scope: SandboxScope::Chat,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert_eq!(
            router.effective_sandbox_key("telegram:lovely:-5288040422"),
            "telegram:chat:-5288040422"
        );
    }

    #[test]
    fn test_effective_sandbox_key_chat_scope_telegram_dm_falls_back_to_session() {
        let config = SandboxConfig {
            scope: SandboxScope::Chat,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        let sk = "telegram:lovely:dm:8454363355";
        assert_eq!(router.effective_sandbox_key(sk), sk);
    }

    #[test]
    fn test_effective_sandbox_key_bot_scope() {
        let config = SandboxConfig {
            scope: SandboxScope::Bot,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert_eq!(
            router.effective_sandbox_key("telegram:lovely:-5288040422"),
            "telegram:bot:lovely"
        );
    }

    #[test]
    fn test_effective_sandbox_key_global_scope() {
        let config = SandboxConfig {
            scope: SandboxScope::Global,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        assert_eq!(router.effective_sandbox_key("any:thing"), "global");
    }

    #[tokio::test]
    async fn test_prune_idle_respects_leases() {
        let backend = Arc::new(TestSandbox::new("test", None, None));
        let config = SandboxConfig {
            scope: SandboxScope::Global,
            idle_ttl_secs: 1,
            ..Default::default()
        };
        let backend_dyn: Arc<dyn Sandbox> = backend.clone();
        let router = SandboxRouter::with_backend(config, backend_dyn);

        let effective_key = router.effective_sandbox_key("main");
        {
            let mut last = router
                .last_used_ms
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            last.insert(effective_key.clone(), 0);
        }

        let lease = router.acquire_lease("main");
        router.prune_idle().await;
        assert_eq!(backend.cleanup_calls.load(Ordering::SeqCst), 0);

        drop(lease);
        router.prune_idle().await;
        assert_eq!(backend.cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_resolve_image_default() {
        let config = SandboxConfig::default();
        let router = SandboxRouter::new(config);
        let img = router.resolve_image("main", None).await;
        assert_eq!(img, DEFAULT_SANDBOX_IMAGE);
    }

    #[tokio::test]
    async fn test_resolve_image_skill_override() {
        let config = SandboxConfig::default();
        let router = SandboxRouter::new(config);
        let img = router
            .resolve_image("main", Some("moltis-cache/my-skill:abc123"))
            .await;
        assert_eq!(img, "moltis-cache/my-skill:abc123");
    }

    #[tokio::test]
    async fn test_resolve_image_session_override() {
        let config = SandboxConfig {
            scope: SandboxScope::Session,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        router
            .set_image_override("sess1", "custom:latest".into())
            .await;
        let img = router.resolve_image("sess1", None).await;
        assert_eq!(img, "custom:latest");
    }

    #[tokio::test]
    async fn test_resolve_image_skill_beats_session() {
        let config = SandboxConfig {
            scope: SandboxScope::Session,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        router
            .set_image_override("sess1", "custom:latest".into())
            .await;
        let img = router
            .resolve_image("sess1", Some("moltis-cache/skill:hash"))
            .await;
        assert_eq!(img, "moltis-cache/skill:hash");
    }

    #[tokio::test]
    async fn test_resolve_image_config_override() {
        let config = SandboxConfig {
            image: Some("my-org/image:v1".into()),
            ..Default::default()
        };
        let router = SandboxRouter::new(config);
        let img = router.resolve_image("main", None).await;
        assert_eq!(img, "my-org/image:v1");
    }

    #[tokio::test]
    async fn test_remove_image_override() {
        let config = SandboxConfig::default();
        let router = SandboxRouter::new(config);
        router
            .set_image_override("sess1", "custom:latest".into())
            .await;
        router.remove_image_override("sess1").await;
        let img = router.resolve_image("sess1", None).await;
        assert_eq!(img, DEFAULT_SANDBOX_IMAGE);
    }

    #[test]
    fn test_docker_image_tag_deterministic() {
        let packages = vec!["curl".into(), "git".into(), "wget".into()];
        let tag1 = sandbox_image_tag("moltis-main-sandbox", "ubuntu:25.10", &packages);
        let tag2 = sandbox_image_tag("moltis-main-sandbox", "ubuntu:25.10", &packages);
        assert_eq!(tag1, tag2);
        assert!(tag1.starts_with("moltis-main-sandbox:"));
    }

    #[test]
    fn test_docker_image_tag_order_independent() {
        let p1 = vec!["curl".into(), "git".into()];
        let p2 = vec!["git".into(), "curl".into()];
        assert_eq!(
            sandbox_image_tag("moltis-main-sandbox", "ubuntu:25.10", &p1),
            sandbox_image_tag("moltis-main-sandbox", "ubuntu:25.10", &p2),
        );
    }

    #[test]
    fn test_docker_image_tag_changes_with_base() {
        let packages = vec!["curl".into()];
        let t1 = sandbox_image_tag("moltis-main-sandbox", "ubuntu:25.10", &packages);
        let t2 = sandbox_image_tag("moltis-main-sandbox", "ubuntu:24.04", &packages);
        assert_ne!(t1, t2);
    }

    #[test]
    fn test_docker_image_tag_changes_with_packages() {
        let p1 = vec!["curl".into()];
        let p2 = vec!["curl".into(), "git".into()];
        let t1 = sandbox_image_tag("moltis-main-sandbox", "ubuntu:25.10", &p1);
        let t2 = sandbox_image_tag("moltis-main-sandbox", "ubuntu:25.10", &p2);
        assert_ne!(t1, t2);
    }

    #[tokio::test]
    async fn test_no_sandbox_build_image_is_noop() {
        let sandbox = NoSandbox;
        let result = sandbox
            .build_image("ubuntu:25.10", &["curl".into()])
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_sandbox_router_events() {
        let config = SandboxConfig::default();
        let router = SandboxRouter::new(config);
        let mut rx = router.subscribe_events();

        router.emit_event(SandboxEvent::Provisioning {
            container: "test".into(),
            packages: vec!["curl".into()],
        });

        let event = rx.try_recv().unwrap();
        match event {
            SandboxEvent::Provisioning {
                container,
                packages,
            } => {
                assert_eq!(container, "test");
                assert_eq!(packages, vec!["curl".to_string()]);
            },
            _ => panic!("unexpected event variant"),
        }
    }

    #[tokio::test]
    async fn test_sandbox_router_global_image_override() {
        let config = SandboxConfig {
            scope: SandboxScope::Session,
            ..Default::default()
        };
        let router = SandboxRouter::new(config);

        // Default
        let img = router.default_image().await;
        assert_eq!(img, DEFAULT_SANDBOX_IMAGE);

        // Set global override
        router
            .set_global_image(Some("moltis-sandbox:abc123".into()))
            .await;
        let img = router.default_image().await;
        assert_eq!(img, "moltis-sandbox:abc123");

        // Global override flows through resolve_image
        let img = router.resolve_image("main", None).await;
        assert_eq!(img, "moltis-sandbox:abc123");

        // Session override still wins
        router.set_image_override("main", "custom:v1".into()).await;
        let img = router.resolve_image("main", None).await;
        assert_eq!(img, "custom:v1");

        // Clear and revert
        router.set_global_image(None).await;
        router.remove_image_override("main").await;
        let img = router.default_image().await;
        assert_eq!(img, DEFAULT_SANDBOX_IMAGE);
    }

    /// When Docker is available, test that we can explicitly select it.
    #[test]
    fn test_select_backend_explicit_choices() {
        // Docker backend
        if is_cli_available("docker") {
            let config = SandboxConfig {
                backend: "docker".into(),
                ..Default::default()
            };
            let backend = select_backend(config);
            assert_eq!(backend.backend_name(), "docker");
        }
    }

    #[tokio::test]
    async fn test_explicit_apple_container_backend_is_unsupported() {
        let config = SandboxConfig {
            backend: "apple-container".into(),
            ..Default::default()
        };
        let backend = select_backend(config);
        let id = SandboxId {
            scope: SandboxScope::Session,
            key: "session-abc".into(),
        };
        let error = backend.ensure_ready(&id, None).await.unwrap_err();
        assert!(format!("{error:#}").contains("SANDBOX_BACKEND_UNSUPPORTED"));
    }

    #[test]
    fn test_is_debian_host() {
        let result = is_debian_host();
        // On macOS/Windows this should be false; on Debian/Ubuntu it should be true.
        if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
            assert!(!result);
        }
        // On Linux, it depends on the distro — just verify it returns a bool without panic.
        let _ = result;
    }

    #[test]
    fn test_host_package_name_candidates_t64_to_base() {
        assert_eq!(
            host_package_name_candidates("libgtk-3-0t64"),
            vec!["libgtk-3-0t64".to_string(), "libgtk-3-0".to_string()]
        );
    }

    #[test]
    fn test_host_package_name_candidates_base_to_t64_for_soname() {
        assert_eq!(
            host_package_name_candidates("libcups2"),
            vec!["libcups2".to_string(), "libcups2t64".to_string()]
        );
    }

    #[test]
    fn test_host_package_name_candidates_non_library_stays_single() {
        assert_eq!(
            host_package_name_candidates("curl"),
            vec!["curl".to_string()]
        );
        assert_eq!(
            host_package_name_candidates("libreoffice-core"),
            vec!["libreoffice-core".to_string()]
        );
    }

    #[tokio::test]
    async fn test_provision_host_packages_empty() {
        let result = provision_host_packages(&[]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_provision_host_packages_non_debian() {
        if is_debian_host() {
            // Can't test the non-debian path on a Debian host.
            return;
        }
        let result = provision_host_packages(&["curl".into()]).await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_is_running_as_root() {
        // In CI and dev, we typically don't run as root.
        let result = is_running_as_root();
        // Just verify it returns a bool without panic.
        let _ = result;
    }

    #[test]
    fn test_should_use_docker_backend() {
        assert!(should_use_docker_backend(true, true));
        assert!(!should_use_docker_backend(true, false));
        assert!(!should_use_docker_backend(false, true));
        assert!(!should_use_docker_backend(false, false));
    }

    #[cfg(target_os = "linux")]
    mod linux_tests {
        use super::*;

        #[test]
        fn test_cgroup_scope_name() {
            let config = SandboxConfig::default();
            let cgroup = CgroupSandbox::new(config);
            let id = SandboxId {
                scope: SandboxScope::Session,
                key: "sess1".into(),
            };
            assert_eq!(cgroup.scope_name(&id), "moltis-sandbox-sess1");
        }

        #[test]
        fn test_cgroup_property_args() {
            let config = SandboxConfig {
                resource_limits: ResourceLimits {
                    memory_limit: Some("1G".into()),
                    cpu_quota: Some(2.0),
                    pids_max: Some(200),
                },
                ..Default::default()
            };
            let cgroup = CgroupSandbox::new(config);
            let args = cgroup.property_args();
            assert!(args.contains(&"MemoryMax=1G".to_string()));
            assert!(args.contains(&"CPUQuota=200%".to_string()));
            assert!(args.contains(&"TasksMax=200".to_string()));
        }
    }
}
