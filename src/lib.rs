use anyhow::Context;
use anyhow::Result;
use log::debug;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::args::*;
use crate::config::*;
use crate::registration::bluesky_register;
use crate::registration::mastodon_register;

pub mod args;
mod config;
mod registration;

pub async fn run(args: Args) -> Result<()> {
    debug!("running with args {:?}", args);

    let config = match fs::read_to_string(&args.config).await {
        Ok(config) => config_load(&config)?,
        Err(_) => {
            let mastodon_config = mastodon_register()
                .await
                .context("Failed to setup mastodon account")?;
            let bluesky_config = bluesky_register()
                .await
                .context("Failed to setup twitter account")?;
            let config = Config {
                mastodon: mastodon_config,
                bluesky: bluesky_config,
            };

            // Save config for using on the next run.
            let toml = toml::to_string(&config)?;
            let mut file = File::create(&args.config)
                .await
                .context("Failed to create config file")?;
            file.write_all(toml.as_bytes()).await?;

            config
        }
    };

    dbg!(config);

    Ok(())
}
