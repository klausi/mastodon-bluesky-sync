use anyhow::Context;
use anyhow::Result;
use bsky_sdk::api::types::LimitedNonZeroU8;
use bsky_sdk::BskyAgent;
use log::debug;
use megalodon::generator;
use megalodon::megalodon::GetAccountStatusesInputOptions;
use std::process;
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

    let client = generator(
        megalodon::SNS::Mastodon,
        config.mastodon.base_url,
        Some(config.mastodon.access_token),
        None,
    );
    let account = match client.verify_account_credentials().await {
        Ok(account) => account,
        Err(e) => {
            eprintln!("Error connecting to Mastodon: {e:#?}");
            process::exit(1);
        }
    };
    // Get most recent 50 toots, exclude replies for now.
    let mastodon_statuses = match client
        .get_account_statuses(
            account.json.id,
            Some(&GetAccountStatusesInputOptions {
                limit: Some(50),
                pinned: Some(false),
                exclude_replies: Some(true),
                exclude_reblogs: Some(!config.mastodon.sync_reblogs),
                only_public: Some(true),
                ..Default::default()
            }),
        )
        .await
    {
        Ok(statuses) => statuses,
        Err(e) => {
            eprintln!("Error fetching toots from Mastodon: {e:#?}");
            process::exit(2);
        }
    };

    let bsky_agent = BskyAgent::builder()
        .config(config.bluesky.bluesky_config)
        .build()
        .await?;
    let bsky_session = bsky_agent.api.com.atproto.server.get_session().await?;
    let bsky_statuses = match bsky_agent
        .api
        .app
        .bsky
        .feed
        .get_author_feed(
            bsky_sdk::api::app::bsky::feed::get_author_feed::ParametersData {
                actor: bsky_session.did.clone().into(),
                cursor: None,
                filter: None,
                include_pins: None,
                limit: Some(LimitedNonZeroU8::try_from(50).unwrap()),
            }
            .into(),
        )
        .await
    {
        Ok(statuses) => statuses,
        Err(e) => {
            eprintln!("Error fetching posts from Bluesky: {e:#?}");
            process::exit(3);
        }
    };

    dbg!(bsky_statuses);

    Ok(())
}
