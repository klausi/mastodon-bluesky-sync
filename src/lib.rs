use anyhow::Context;
use anyhow::Result;
use bsky_sdk::agent::config::FileStore;
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
use crate::post::*;
use crate::registration::bluesky_register;
use crate::registration::mastodon_register;
use crate::sync::*;

pub mod args;
mod config;
mod post;
mod registration;
mod sync;

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
                .context("Failed to setup Bluesky account")?;
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

    let mastodon = generator(
        megalodon::SNS::Mastodon,
        config.mastodon.base_url.clone(),
        Some(config.mastodon.access_token.clone()),
        None,
    );
    let account = match mastodon.verify_account_credentials().await {
        Ok(account) => account,
        Err(e) => {
            eprintln!("Error connecting to Mastodon: {e:#?}");
            process::exit(1);
        }
    };
    // Get most recent 50 toots, exclude replies for now.
    let mastodon_statuses = match mastodon
        .get_account_statuses(
            account.json.id,
            Some(&GetAccountStatusesInputOptions {
                limit: Some(1),
                pinned: Some(false),
                exclude_replies: Some(true),
                exclude_reblogs: Some(!config.mastodon.sync_reblogs),
                only_public: Some(true),
                ..Default::default()
            }),
        )
        .await
    {
        Ok(statuses) => statuses.json,
        Err(e) => {
            eprintln!("Error fetching toots from Mastodon: {e:#?}");
            process::exit(2);
        }
    };

    // First try to login with a cached access token.
    let bsky_agent =
        match bsky_sdk::agent::config::Config::load(&FileStore::new("bluesky-auth-cache.json"))
            .await
        {
            Ok(bsky_config) => match BskyAgent::builder().config(bsky_config).build().await {
                Ok(agent) => agent,
                Err(_) => {
                    get_new_bluesky_agent(&config.bluesky.email, &config.bluesky.app_password)
                        .await?
                }
            },
            Err(_) => {
                get_new_bluesky_agent(&config.bluesky.email, &config.bluesky.app_password).await?
            }
        };
    let bsky_session = bsky_agent
        .api
        .com
        .atproto
        .server
        .get_session()
        .await
        .context("Error getting Bluesky session")?;
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
                limit: Some(LimitedNonZeroU8::try_from(1).unwrap()),
            }
            .into(),
        )
        .await
    {
        Ok(statuses) => statuses.feed.clone(),
        Err(e) => {
            eprintln!("Error fetching posts from Bluesky: {e:#?}");
            process::exit(3);
        }
    };

    let options = SyncOptions {
        sync_reblogs: config.mastodon.sync_reblogs,
        sync_reskeets: config.bluesky.sync_reskeets,
        sync_hashtag_mastodon: config.mastodon.sync_hashtag,
        sync_hashtag_bluesky: config.bluesky.sync_hashtag,
    };

    let mut posts = determine_posts(&mastodon_statuses, &bsky_statuses, &options);

    // Prevent double posting with a post cache that records each new status
    // message.
    let post_cache_file = &cache_file("post_cache.json");
    let mut post_cache = read_post_cache(post_cache_file);
    let mut cache_changed = false;
    posts = filter_posted_before(posts, &post_cache)?;

    for toot in posts.toots {
        if !args.skip_existing_posts {
            if let Err(e) = post_to_mastodon(&mastodon, &toot, args.dry_run).await {
                eprintln!("Error posting toot to Mastodon: {e:#?}");
                continue;
            }
        }
        // Posting API call was successful: store text in cache to prevent any
        // double posting next time.
        if !args.dry_run {
            post_cache.insert(toot.text);
            cache_changed = true;
        }
    }

    for post in posts.bsky_posts {
        if !args.skip_existing_posts {
            if let Err(e) = post_to_bluesky(&bsky_agent, &post, args.dry_run).await {
                eprintln!("Error posting to Bluesky: {e:#?}");
                continue;
            }
        }
        // Posting API call was successful: store text in cache to prevent any
        // double posting next time.
        if !args.dry_run {
            post_cache.insert(post.text);
            cache_changed = true;
        }
    }

    // Write out the cache file if necessary.
    if !args.dry_run && cache_changed {
        let json = serde_json::to_string_pretty(&post_cache)?;
        fs::write(post_cache_file, json.as_bytes()).await?;
    }

    Ok(())
}

/// Returns the full path for a cache file name.
fn cache_file(name: &str) -> String {
    if let Ok(cache_dir) = std::env::var("MBS_CACHE_DIR") {
        return format!("{cache_dir}/{name}");
    }
    name.into()
}

async fn get_new_bluesky_agent(email: &str, app_password: &str) -> Result<BskyAgent> {
    let agent = BskyAgent::builder().build().await?;
    let _session = agent.login(email, app_password).await?;
    agent
        .to_config()
        .await
        .save(&FileStore::new("bluesky-auth-cache.json"))
        .await?;
    Ok(agent)
}
