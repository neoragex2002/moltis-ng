# Sandbox Backends

Moltis runs LLM-generated commands inside containers to protect your host
system. The sandbox backend controls which container technology is used.

## Backend Selection

Configure in `moltis.toml`:

```toml
[tools.exec.sandbox]
backend = "auto"          # default — picks the best available
# backend = "docker"      # force Docker
# backend = "apple-container"  # not supported (will fail-fast)
```

With `"auto"` (the default), Moltis uses Docker when available:

| Priority | Backend           | Platform | Isolation          |
|----------|-------------------|----------|--------------------|
| 1        | Docker            | any      | Linux namespaces / cgroups    |
| 2        | none (host)       | any      | no isolation                  |

## Data directory mount (`/moltis/data`)

Sandbox containers use a fixed, stable path for the Moltis instance data
directory: `/moltis/data`.

On Docker, this requires configuring the mount backing explicitly:

```toml
[tools.exec.sandbox]
data_mount = "ro"                 # "ro" | "rw" (must not be "none" when sandboxing)
data_mount_type = "bind"          # "bind" | "volume"
data_mount_source = "/srv/moltis-data" # bind: absolute host path | volume: volume name
```

Moltis injects `MOLTIS_DATA_DIR=/moltis/data` into sandbox containers so any code
running inside the sandbox resolves the data directory consistently.

## Docker

Docker is supported on macOS, Linux, and Windows. On macOS it runs inside a
Linux VM managed by Docker Desktop.

Install from https://docs.docker.com/get-docker/

## No sandbox

If neither runtime is found, commands execute directly on the host. The
startup banner will show a warning. This is **not recommended** for untrusted
workloads.

## Per-session overrides

The web UI allows toggling sandboxing per session and selecting a custom
container image. These overrides persist across gateway restarts.

## Resource limits

```toml
[tools.exec.sandbox.resource_limits]
memory_limit = "512M"
cpu_quota = 1.0
pids_max = 256
```
