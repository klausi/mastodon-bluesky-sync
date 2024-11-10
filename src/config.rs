use anyhow::Result;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::NoneAsEmptyString;
use std::collections::BTreeMap;
use tokio::fs;
use tokio::fs::remove_file;

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
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct BlueskyConfig {
    pub email: String,
    pub app_password: String,
    #[serde(default = "config_true_default")]
    pub sync_reskeets: bool,
    #[serde_as(as = "NoneAsEmptyString")]
    #[serde(default = "config_none_default")]
    pub sync_hashtag: Option<String>,
}

fn config_true_default() -> bool {
    true
}

fn config_none_default<T>() -> Option<T> {
    None
}

pub async fn load_dates_from_cache(
    cache_file: &str,
) -> Result<Option<BTreeMap<DateTime<Utc>, u64>>> {
    if let Ok(json) = fs::read_to_string(cache_file).await {
        let cache = serde_json::from_str(&json)?;
        Ok(Some(cache))
    } else {
        Ok(None)
    }
}

pub async fn save_dates_to_cache(
    cache_file: &str,
    dates: &BTreeMap<DateTime<Utc>, u64>,
) -> Result<()> {
    let json = serde_json::to_string_pretty(&dates)?;
    fs::write(cache_file, json.as_bytes()).await?;
    Ok(())
}

// Delete a list of dates from the given cache of dates and write the cache to
// disk if necessary.
pub async fn remove_dates_from_cache(
    remove_dates: Vec<&DateTime<Utc>>,
    cached_dates: &BTreeMap<DateTime<Utc>, u64>,
    cache_file: &str,
) -> Result<()> {
    if remove_dates.is_empty() {
        return Ok(());
    }

    let mut new_dates = cached_dates.clone();
    for remove_date in remove_dates {
        new_dates.remove(remove_date);
    }

    if new_dates.is_empty() {
        // If we have deleted all old dates from our cache file we can remove
        // it. On the next run all entries will be fetched and the cache
        // recreated.
        remove_file(cache_file).await?;
    } else {
        save_dates_to_cache(cache_file, &new_dates).await?;
    }

    Ok(())
}
