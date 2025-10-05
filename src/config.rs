use anyhow::Result;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use serde_with::NoneAsEmptyString;
use serde_with::serde_as;
use std::collections::BTreeMap;
use tokio::fs;
use tokio::fs::remove_file;

pub type DatePostList = BTreeMap<String, DateTime<Utc>>;

#[inline]
pub fn config_load(config: &str) -> Result<Config> {
    toml::from_str(config).map_err(anyhow::Error::from)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub mastodon: MastodonConfig,
    pub bluesky: BlueskyConfig,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct MastodonConfig {
    pub base_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "config_true_default")]
    pub sync_reblogs: bool,
    #[serde_as(as = "NoneAsEmptyString")]
    #[serde(default = "config_none_default")]
    pub sync_hashtag: Option<String>,
    #[serde(default = "config_false_default")]
    pub delete_old_favs: bool,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct BlueskyConfig {
    pub email: String,
    pub app_password: String,
    #[serde(default = "config_true_default")]
    pub sync_reposts: bool,
    #[serde_as(as = "NoneAsEmptyString")]
    #[serde(default = "config_none_default")]
    pub sync_hashtag: Option<String>,
    #[serde(default = "config_false_default")]
    pub delete_old_posts: bool,
    #[serde(default = "config_false_default")]
    pub delete_old_favs: bool,
}

fn config_true_default() -> bool {
    true
}

fn config_none_default<T>() -> Option<T> {
    None
}

fn config_false_default() -> bool {
    false
}

pub async fn remove_date_from_cache(post_id: &str, cache_file: &str) -> Result<()> {
    let dates_cache = load_dates_from_cache(cache_file).await?;
    if let Some(mut dates) = dates_cache {
        dates.remove(post_id);
        save_dates_to_cache(cache_file, &dates).await?;
    }

    Ok(())
}

pub async fn load_dates_from_cache(cache_file: &str) -> Result<Option<DatePostList>> {
    if let Ok(json) = fs::read_to_string(cache_file).await {
        let cache = serde_json::from_str(&json)?;
        Ok(Some(cache))
    } else {
        Ok(None)
    }
}

pub async fn save_dates_to_cache(cache_file: &str, dates: &DatePostList) -> Result<()> {
    if dates.is_empty() {
        // If the cache file exists delete it.
        if fs::metadata(cache_file).await.is_ok() {
            remove_file(cache_file).await?;
        }
        return Ok(());
    }
    let json = serde_json::to_string_pretty(&dates)?;
    fs::write(cache_file, json.as_bytes()).await?;
    Ok(())
}
