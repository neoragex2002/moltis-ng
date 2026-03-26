# Exec Sandbox (Docker-only)

Moltis runs LLM-generated `exec` / `process` commands inside Docker containers
to isolate untrusted workloads from your host system.

This page documents the **exec sandbox** (`[tools.exec.sandbox]`), not the
browser tool’s container settings.

## Configuration

`moltis.toml`:

```toml
[tools.exec.sandbox]
# When to apply sandboxing:
# - "off": no container isolation (runs on host)
# - "non-main": sandbox only for non-main sessions
# - "all": sandbox for all sessions
mode = "off"

# Sandbox runtime image. Must already exist in your local Docker image store.
# Moltis does NOT build or pull images.
# image = "moltis-sandbox:2026-03-26"

# Startup policy for managed sandbox containers:
# - "reset": delete managed containers at startup
# - "reuse": keep only containers matching the current contract
startup_container_policy = "reset"

# Container reuse boundary:
# - "session_id": per session instance
# - "session_key": per logical session bucket
scope_key = "session_id"

# Idle TTL for sandbox containers (seconds). 0 disables TTL.
idle_ttl_secs = 0

# Data directory mount contract (required when mode != "off")
data_mount = "ro"           # "ro" | "rw"
data_mount_type = "bind"    # "bind" | "volume"
data_mount_source = "/srv/moltis-data"
```

## One-cut rules (no backward compatibility)

- Docker-only: when `mode != "off"`, Docker must be available or the gateway
  fails fast (no host fallback).
- Single runtime image: `tools.exec.sandbox.image` is the **only** runtime image
  source. There are no per-session overrides.
- No build/pull/provision: Moltis does not `docker build`, does not pull, and
  does not apt-get install packages at runtime.

## Container paths (runtime contract)

Sandbox containers use two fixed guest paths:

- `/moltis/data`: Moltis instance data directory (mounted read-only or read-write)
- `/moltis/workdir`: the only default writable working directory

Moltis also enforces:

- `-w /moltis/workdir`
- `HOME=/moltis/workdir`
- `TMPDIR=/moltis/workdir/tmp`

## Data directory mount (`/moltis/data`)

Sandbox containers use a fixed, stable path for the Moltis instance data
directory: `/moltis/data`.

On Docker, this requires configuring the mount backing explicitly:

```toml
[tools.exec.sandbox]
data_mount = "ro"                  # "ro" | "rw"
data_mount_type = "bind"           # "bind" | "volume"
data_mount_source = "/srv/moltis-data" # bind: absolute host path | volume: volume name
```

Moltis injects `MOLTIS_DATA_DIR=/moltis/data` into sandbox containers so any code
running inside the sandbox resolves the data directory consistently.

## External mounts (deny-by-default)

Additional host mounts can be configured with `mounts[]`, but are deny-by-default.
If `mount_allowlist` is empty, all external mounts are rejected.

```toml
[tools.exec.sandbox]
mount_allowlist = ["/srv"]
mounts = [
  { host_dir = "/srv/shared", guest_dir = "/mnt/shared", mode = "ro" },
]
```

## Troubleshooting

- `SANDBOX_BACKEND_UNAVAILABLE`: Docker daemon not reachable.
- `SANDBOX_IMAGE_MISSING`: the configured image is not in the local image store.
  Verify with `docker image inspect <image>`.
- `SANDBOX_IMAGE_CONTRACT_INVALID`: the image exists but fails the runtime contract
  checks (workdir/env/exec).

## Resource limits

```toml
[tools.exec.sandbox.resource_limits]
memory_limit = "512M"
cpu_quota = 1.0
pids_max = 256
```
