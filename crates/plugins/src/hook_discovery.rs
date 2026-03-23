//! Hook discovery from filesystem directories.
//!
//! Scans configured directories for hook definitions (`HOOK.md` files)
//! and produces [`ParsedHook`] entries.

use std::path::PathBuf;

use {async_trait::async_trait, tracing::warn};

use crate::hook_metadata::{ParsedHook, parse_hook_md};

/// Source of a discovered hook.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookSource {
    Project,
    User,
    Bundled,
}

/// Discovers hooks from the filesystem.
#[async_trait]
pub trait HookDiscoverer: Send + Sync {
    /// Scan configured paths and return all discovered hooks.
    async fn discover(&self) -> anyhow::Result<Vec<(ParsedHook, HookSource)>>;
}

/// Filesystem-based hook discoverer. Scans directories in priority order.
pub struct FsHookDiscoverer {
    search_paths: Vec<(PathBuf, HookSource)>,
}

impl FsHookDiscoverer {
    pub fn new(search_paths: Vec<(PathBuf, HookSource)>) -> Self {
        Self { search_paths }
    }

    /// Build the default search paths for hook discovery.
    ///
    /// Project-local hooks live under `<cwd>/.moltis/hooks`; user-global hooks
    /// live under the configured data directory.
    pub fn default_paths() -> Vec<(PathBuf, HookSource)> {
        default_paths_with(
            moltis_config::project_local_dir(),
            moltis_config::data_dir(),
        )
    }
}

fn default_paths_with(project_root: PathBuf, data_dir: PathBuf) -> Vec<(PathBuf, HookSource)> {
    vec![
        (project_root.join("hooks"), HookSource::Project),
        (data_dir.join("hooks"), HookSource::User),
    ]
}

#[async_trait]
impl HookDiscoverer for FsHookDiscoverer {
    async fn discover(&self) -> anyhow::Result<Vec<(ParsedHook, HookSource)>> {
        let mut hooks = Vec::new();

        for (base_path, source) in &self.search_paths {
            if !base_path.is_dir() {
                continue;
            }

            let entries = match std::fs::read_dir(base_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let hook_dir = entry.path();
                if !hook_dir.is_dir() {
                    continue;
                }

                let hook_md = hook_dir.join("HOOK.md");
                if !hook_md.is_file() {
                    continue;
                }

                let content = match std::fs::read_to_string(&hook_md) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(?hook_md, %e, "failed to read HOOK.md");
                        continue;
                    },
                };

                match parse_hook_md(&content, &hook_dir) {
                    Ok(parsed) => hooks.push((parsed, source.clone())),
                    Err(e) => warn!(?hook_dir, %e, "failed to parse HOOK.md"),
                }
            }
        }

        Ok(hooks)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discover_hooks_in_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks_dir = tmp.path().join("hooks");
        std::fs::create_dir_all(hooks_dir.join("my-hook")).unwrap();
        std::fs::write(
            hooks_dir.join("my-hook/HOOK.md"),
            r#"+++
name = "my-hook"
description = "test"
events = ["SessionStart"]
command = "./handler.sh"
+++
body
"#,
        )
        .unwrap();

        let discoverer = FsHookDiscoverer::new(vec![(hooks_dir.clone(), HookSource::Project)]);
        let hooks = discoverer.discover().await.unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].0.metadata.name, "my-hook");
        assert_eq!(hooks[0].1, HookSource::Project);
    }

    #[tokio::test]
    async fn discover_skips_missing_dirs() {
        let discoverer =
            FsHookDiscoverer::new(vec![(PathBuf::from("/nonexistent"), HookSource::User)]);
        let hooks = discoverer.discover().await.unwrap();
        assert!(hooks.is_empty());
    }

    #[tokio::test]
    async fn discover_skips_dirs_without_hook_md() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks_dir = tmp.path().join("hooks");
        std::fs::create_dir_all(hooks_dir.join("not-a-hook")).unwrap();
        std::fs::write(hooks_dir.join("not-a-hook/README.md"), "hello").unwrap();

        let discoverer = FsHookDiscoverer::new(vec![(hooks_dir, HookSource::Project)]);
        let hooks = discoverer.discover().await.unwrap();
        assert!(hooks.is_empty());
    }

    #[tokio::test]
    async fn discover_skips_invalid_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks_dir = tmp.path().join("hooks");
        std::fs::create_dir_all(hooks_dir.join("bad-hook")).unwrap();
        std::fs::write(hooks_dir.join("bad-hook/HOOK.md"), "no frontmatter").unwrap();

        let discoverer = FsHookDiscoverer::new(vec![(hooks_dir, HookSource::Project)]);
        let hooks = discoverer.discover().await.unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn default_paths_use_project_local_and_data_roots() {
        let project_root = PathBuf::from("/tmp/workspace/.moltis");
        let data_dir = PathBuf::from("/tmp/home/.moltis/data");
        let paths = default_paths_with(project_root.clone(), data_dir.clone());

        assert_eq!(paths[0], (project_root.join("hooks"), HookSource::Project));
        assert_eq!(paths[1], (data_dir.join("hooks"), HookSource::User));
    }
}
