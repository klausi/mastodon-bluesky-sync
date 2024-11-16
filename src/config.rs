use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::NoneAsEmptyString;

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
    pub sync_reposts: bool,
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
