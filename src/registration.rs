use anyhow::{Ok, Result};
use bsky_sdk::{agent::config::FileStore, BskyAgent};
use megalodon::generator;
use std::io;

use super::*;

pub async fn mastodon_register() -> Result<MastodonConfig> {
    let base_url = console_input(
        "Provide the URL of your Mastodon instance, for example https://mastodon.social ",
    )?;
    let client = generator(megalodon::SNS::Mastodon, base_url.clone(), None, None);
    let options = megalodon::megalodon::AppInputOptions {
        scopes: Some(["read".to_string(), "write".to_string()].to_vec()),
        website: Some("https://github.com/klausi/mastodon-bluesky-sync".to_string()),
        ..Default::default()
    };

    let app_data = client
        .register_app("Mastodon Bluesky Sync".to_string(), &options)
        .await?;
    let client_id = app_data.client_id;
    let client_secret = app_data.client_secret;
    println!("Click this link to authorize on Mastodon:");
    println!("{}", app_data.url.unwrap());

    let code = console_input("Enter authorization code from website")?;

    let token_data = client
        .fetch_access_token(
            client_id.clone(),
            client_secret.clone(),
            code.trim().to_string(),
            megalodon::default::NO_REDIRECT.to_string(),
        )
        .await?;

    Ok(MastodonConfig {
        base_url,
        client_id,
        client_secret,
        access_token: token_data.access_token,
        refresh_token: token_data.refresh_token.unwrap_or("none".to_string()),
        sync_reblogs: true,
        sync_hashtag: None,
    })
}

pub async fn bluesky_register() -> Result<BlueskyConfig> {
    let email = console_input("Enter your Bluesky email address")?;
    let app_password = console_input("Generate a Bluesky App password at https://bsky.app/settings/app-passwords and paste it here")?;
    let _agent = get_new_bluesky_agent(&email, &app_password).await?;
    // Bluesky access tokens do not work for longer periods of time, so we need
    // to store an app password here.
    // See https://github.com/sugyan/atrium/issues/246
    Ok(BlueskyConfig {
        email,
        app_password,
        sync_reskeets: true,
        sync_hashtag: None,
    })
}

fn console_input(prompt: &str) -> Result<String> {
    println!("{prompt}: ");
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
