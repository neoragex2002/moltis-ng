use std::collections::HashMap;
#[cfg(windows)]
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{sync::Arc, sync::RwLock};

use anyhow::Context;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_FILE_NAME: &str = "models.dev.api.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAiModelLimits {
    pub context: u32,
    pub input: Option<u32>,
    pub output: u32,
}

#[derive(Debug, serde::Deserialize)]
struct ModelsDevProvider {
    id: String,

    #[serde(default)]
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelsDevModel {
    id: String,

    #[allow(dead_code)]
    name: String,

    limit: ModelsDevLimit,
}

#[derive(Debug, serde::Deserialize)]
struct ModelsDevLimit {
    context: u32,

    #[serde(default)]
    input: Option<u32>,

    output: u32,
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_FILE_NAME)
}

fn cache_is_fresh(path: &Path) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let modified = match meta.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let now = SystemTime::now();
    let Ok(age) = now.duration_since(modified) else {
        return false;
    };
    age <= CACHE_TTL
}

fn write_snapshot_atomic(path: &Path, body: &str) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create models.dev cache dir: {}", parent.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("tmp-{}-{nanos}", std::process::id()));
    std::fs::write(&tmp, body.as_bytes())
        .with_context(|| format!("write models.dev cache temp file: {}", tmp.display()))?;

    if let Err(err) = replace_file(&tmp, path) {
        // Best-effort cleanup — leaving a temp file is not fatal but is noisy.
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

fn replace_file(tmp: &Path, path: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        // On Windows, `rename(tmp, path)` fails if `path` already exists.
        match std::fs::rename(tmp, path) {
            Ok(()) => Ok(()),
            Err(first_err) => {
                let remove = std::fs::remove_file(path);
                if let Err(remove_err) = remove
                    && remove_err.kind() != io::ErrorKind::NotFound
                {
                    return Err(remove_err).with_context(|| {
                        format!("remove existing models.dev cache file: {}", path.display())
                    });
                }
                std::fs::rename(tmp, path).with_context(|| {
                    format!(
                        "rename models.dev cache file (after remove); first_error={first_err}: {}",
                        path.display()
                    )
                })?;
                Ok(())
            }
        }
    }

    #[cfg(not(windows))]
    {
        std::fs::rename(tmp, path)
            .with_context(|| format!("rename models.dev cache file: {}", path.display()))?;
        Ok(())
    }
}

fn load_cached_root(data_dir: &Path) -> anyhow::Result<HashMap<String, ModelsDevProvider>> {
    let path = cache_path(data_dir);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read models.dev cache: {}", path.display()))?;
    let root: HashMap<String, ModelsDevProvider> = serde_json::from_str(&raw)
        .with_context(|| format!("parse models.dev cache: {}", path.display()))?;
    Ok(root)
}

fn normalize_openai_model_id(model_id: &str) -> &str {
    let model_id = model_id
        .rsplit_once("::")
        .map(|(_, raw)| raw)
        .unwrap_or(model_id);
    model_id.strip_prefix("openai/").unwrap_or(model_id)
}

fn limits_for_openai_model_in_root(
    root: &HashMap<String, ModelsDevProvider>,
    model_id: &str,
) -> Option<OpenAiModelLimits> {
    let provider = root.get("openai")?;
    if provider.id != "openai" {
        return None;
    }
    let model_id = normalize_openai_model_id(model_id);
    let model = provider.models.get(model_id)?;
    if model.id != model_id {
        return None;
    }
    Some(OpenAiModelLimits {
        context: model.limit.context,
        input: model.limit.input,
        output: model.limit.output,
    })
}

pub fn cached_openai_model_limits(data_dir: &Path, model_id: &str) -> Option<OpenAiModelLimits> {
    struct Memo {
        modified: SystemTime,
        len: u64,
        root: Arc<HashMap<String, ModelsDevProvider>>,
    }

    let path = cache_path(data_dir);
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let len = meta.len();

    static MEMO: std::sync::OnceLock<RwLock<HashMap<PathBuf, Memo>>> = std::sync::OnceLock::new();
    let lock = MEMO.get_or_init(|| RwLock::new(HashMap::new()));
    if let Ok(guard) = lock.read()
        && let Some(ref memo) = guard.get(&path)
        && memo.modified == modified
        && memo.len == len
    {
        return limits_for_openai_model_in_root(&memo.root, model_id);
    }

    let root = load_cached_root(data_dir).ok()?;
    let root = Arc::new(root);
    if let Ok(mut guard) = lock.write() {
        guard.insert(
            path,
            Memo {
                modified,
                len,
                root: root.clone(),
            },
        );
    }

    limits_for_openai_model_in_root(&root, model_id)
}

pub fn spawn_refresh_models_dev_cache(data_dir: PathBuf) {
    if cfg!(test) {
        return;
    }

    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }

    std::thread::spawn(move || {
        let path = cache_path(&data_dir);
        if cache_is_fresh(&path) {
            return;
        }

        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(refresh_models_dev_cache_inner(&data_dir)));

        if let Err(err) = result {
            tracing::warn!(error = %err, "failed to refresh models.dev cache (non-fatal)");
        }
    });
}

async fn refresh_models_dev_cache_inner(data_dir: &Path) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .context("build reqwest client")?;

    let resp = client
        .get(MODELS_DEV_URL)
        .send()
        .await
        .context("fetch models.dev api.json")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("models.dev HTTP {status}");
    }
    let body = resp.text().await.context("read models.dev body")?;

    let root: HashMap<String, ModelsDevProvider> = serde_json::from_str(&body)
        .context("parse models.dev api.json")?;
    if !root.contains_key("openai") {
        anyhow::bail!("models.dev schema missing openai provider");
    }

    write_snapshot_atomic(&cache_path(data_dir), &body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    fn fixture_with_limits(include_input: bool, output: u32) -> String {
        let mut limit = json!({
            "context": 400000,
            "output": output,
        });

        if include_input {
            limit["input"] = json!(272000);
        }

        let root = json!({
            "openai": {
                "id": "openai",
                "name": "OpenAI",
                "models": {
                    "gpt-5.2": {
                        "id": "gpt-5.2",
                        "name": "GPT-5.2",
                        "limit": limit,
                    }
                }
            }
        });

        serde_json::to_string(&root).unwrap()
    }

    #[test]
    fn parse_fixture_gpt_5_2_with_input() {
        let raw = fixture_with_limits(true, 128000);
        let root: HashMap<String, ModelsDevProvider> = serde_json::from_str(&raw).unwrap();
        let limits = limits_for_openai_model_in_root(&root, "gpt-5.2").unwrap();
        assert_eq!(
            limits,
            OpenAiModelLimits {
                context: 400000,
                input: Some(272000),
                output: 128000
            }
        );
    }

    #[test]
    fn parse_fixture_gpt_5_2_missing_input_is_ok() {
        let raw = fixture_with_limits(false, 128000);
        let root: HashMap<String, ModelsDevProvider> = serde_json::from_str(&raw).unwrap();
        let limits = limits_for_openai_model_in_root(&root, "openai/gpt-5.2").unwrap();
        assert_eq!(limits.input, None);
        assert_eq!(limits.context, 400000);
        assert_eq!(limits.output, 128000);
    }

    #[test]
    fn cached_openai_model_limits_reads_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(dir.path());
        write_snapshot_atomic(&cache, &fixture_with_limits(true, 128000)).unwrap();
        let got = cached_openai_model_limits(dir.path(), "gpt-5.2").unwrap();
        assert_eq!(got.output, 128000);
        assert_eq!(got.context, 400000);
    }

    #[test]
    fn write_snapshot_atomic_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(dir.path());

        write_snapshot_atomic(&cache, &fixture_with_limits(true, 111)).unwrap();
        write_snapshot_atomic(&cache, &fixture_with_limits(true, 222)).unwrap();

        let got = cached_openai_model_limits(dir.path(), "gpt-5.2").unwrap();
        assert_eq!(got.output, 222);
    }

    #[test]
    fn cached_openai_model_limits_memoizes_by_cache_path_not_only_mtime() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        let cache1 = cache_path(dir1.path());
        let cache2 = cache_path(dir2.path());

        write_snapshot_atomic(&cache1, &fixture_with_limits(true, 111)).unwrap();
        write_snapshot_atomic(&cache2, &fixture_with_limits(true, 222)).unwrap();

        // Force both cache files to share the same mtime to ensure memoization
        // never cross-pollinates between distinct cache paths.
        let fixed = UNIX_EPOCH + Duration::from_secs(42);

        for cache in [&cache1, &cache2] {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(cache)
                .unwrap();
            let times = std::fs::FileTimes::new().set_modified(fixed);
            file.set_times(times).unwrap();
        }

        let got1 = cached_openai_model_limits(dir1.path(), "gpt-5.2").unwrap();
        let got2 = cached_openai_model_limits(dir2.path(), "gpt-5.2").unwrap();

        assert_eq!(got1.output, 111);
        assert_eq!(got2.output, 222);
    }
}
