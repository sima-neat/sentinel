use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::{CachePayload, DEFAULT_CACHE_DIR, DEFAULT_CACHE_PATH};

pub fn read_cache(path: &str) -> Result<CachePayload> {
    let data = fs::read_to_string(path).with_context(|| format!("read cache {path}"))?;
    serde_json::from_str(&data).with_context(|| format!("parse cache {path}"))
}

pub fn write_cache(path: &str, payload: &CachePayload) -> Result<()> {
    let path = Path::new(path);
    let parent = path
        .parent()
        .unwrap_or_else(|| Path::new(DEFAULT_CACHE_DIR));
    fs::create_dir_all(parent).with_context(|| format!("create cache dir {}", parent.display()))?;
    let tmp = tmp_path(path);
    let data = serde_json::to_vec_pretty(payload).context("serialize cache")?;
    {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(&data).context("write cache data")?;
        file.write_all(b"\n").context("write cache newline")?;
        file.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("cache.json");
    path.with_file_name(format!(".{name}.tmp"))
}

pub fn default_cache_path() -> &'static str {
    DEFAULT_CACHE_PATH
}
