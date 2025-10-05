use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use bsky_sdk::api::types::LimitedNonZeroU8;
use bsky_sdk::api::types::TryFromUnknown;
use bsky_sdk::api::types::string::AtIdentifier;
use bsky_sdk::api::types::string::Nsid;
use bsky_sdk::api::types::string::RecordKey;
use chrono::Duration;
use chrono::prelude::*;
use megalodon::Megalodon;
use megalodon::error::Kind;
use megalodon::megalodon::GetFavouritesInputOptions;
use std::collections::BTreeMap;
use tokio::fs;

use crate::BskyAgent;
use crate::cache_file;
use crate::config::*;

// Delete old favourites of this account that are older than 90 days.
pub async fn mastodon_delete_older_favs(
    mastodon: &(dyn Megalodon + Send + Sync),
    dry_run: bool,
) -> Result<()> {
    // In order not to fetch old favs every time keep them in a cache file
    // keyed by their dates.
    let cache_file = &cache_file("mastodon_fav_cache.json");
    let dates = mastodon_load_fav_dates(mastodon, cache_file).await?;
    let three_months_ago = Utc::now() - Duration::days(90);
    for (toot_id, date) in dates.iter().filter(|(_, date)| date < &&three_months_ago) {
        println!("Deleting Mastodon fav {toot_id} from {date}");
        // Do nothing on a dry run, just print what would be done.
        if dry_run {
            continue;
        }

        match mastodon.unfavourite_status(toot_id.to_string()).await {
            Ok(_) => {
                remove_date_from_cache(toot_id, cache_file).await?;
            }
            Err(error) => {
                if let megalodon::error::Error::OwnError(ref own_error) = error {
                    if let Kind::HTTPStatusError = own_error.kind {
                        if let Some(status) = own_error.status {
                            match status {
                                // The status could have been deleted already by the user, ignore API
                                // errors in that case.
                                404 => {
                                    remove_date_from_cache(toot_id, cache_file).await?;
                                }
                                // Mastodon API rate limit exceeded, stopping fav deletion for now.
                                429 => {
                                    println!(
                                        "Mastodon API rate limit exceeded, stopping fav deletion for now."
                                    );
                                    return Ok(());
                                }
                                _ => return Err(error.into()),
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn mastodon_load_fav_dates(
    mastodon: &(dyn Megalodon + Send + Sync),
    cache_file: &str,
) -> Result<DatePostList> {
    match load_dates_from_cache(cache_file).await? {
        Some(dates) => Ok(dates),
        None => mastodon_fetch_fav_dates(mastodon, cache_file).await,
    }
}

async fn mastodon_fetch_fav_dates(
    mastodon: &(dyn Megalodon + Send + Sync),
    cache_file: &str,
) -> Result<DatePostList> {
    let mut dates = BTreeMap::new();
    let mut max_id = u64::MAX;
    loop {
        println!("Fetching Mastodon favs older than {max_id}");
        let response = mastodon
            .get_favourites(Some(&GetFavouritesInputOptions {
                // Maximum number of statuses to get is 40.
                limit: Some(40),
                max_id: if max_id == u64::MAX {
                    None
                } else {
                    Some(max_id.to_string())
                },
                min_id: None,
            }))
            .await?;
        for status in &response.json {
            dates.insert(status.id.to_string(), status.created_at);
        }
        // Pagination: Parse the Link header to get the next max_id.
        match response.header.get("link") {
            Some(link) => match mastodon_parse_next_max_id(link.to_str()?) {
                Some(new_max_id) => {
                    max_id = new_max_id;
                }
                None => break,
            },
            None => break,
        }
    }

    save_dates_to_cache(cache_file, &dates).await?;

    Ok(dates)
}

// Todo: Megalodon should provide API methods for pagination.
fn mastodon_parse_next_max_id(link_header: &str) -> Option<u64> {
    let re = regex::Regex::new(r#"max_id=(\d+)"#).unwrap();
    if let Some(captures) = re.captures(link_header) {
        if let Some(max_id) = captures.get(1) {
            if let Ok(max_id) = max_id.as_str().parse::<u64>() {
                return Some(max_id);
            }
        }
    }
    None
}

// Delete old favorites of this account that are older than 90 days.
pub async fn bluesky_delete_older_favs(bsky_agent: &BskyAgent, dry_run: bool) -> Result<()> {
    // In order not to fetch old posts every time keep them in a cache file
    // keyed by their dates.
    let cache_file = &cache_file("bluesky_fav_cache.json");
    let dates = bluesky_fetch_fav_dates(bsky_agent, cache_file).await?;
    let three_months_ago = Utc::now() - Duration::days(90);
    let actor: AtIdentifier = bsky_agent.get_session().await.unwrap().did.clone().into();
    for (post_uri, date) in dates.iter().filter(|(_, date)| date < &&three_months_ago) {
        println!("Deleting Bluesky favorite from {date}: {post_uri}");
        // Do nothing on a dry run, just print what would be done.
        if dry_run {
            continue;
        }
        let parts = post_uri
            .strip_prefix("at://")
            .with_context(|| format!("Invalid At URI prefix {post_uri} when deleting fav"))?
            .splitn(3, '/')
            .collect::<Vec<_>>();
        let rkey = match parts[2].parse::<RecordKey>() {
            Ok(rkey) => rkey,
            Err(e) => bail!("Invalid At URI rkey {post_uri} when deleting fav: {e}"),
        };
        bsky_agent
            .api
            .com
            .atproto
            .repo
            .delete_record(
                bsky_sdk::api::com::atproto::repo::delete_record::InputData {
                    collection: Nsid::new("app.bsky.feed.like".to_string()).unwrap(),
                    repo: actor.clone(),
                    rkey: rkey.into(),
                    swap_commit: None,
                    swap_record: None,
                }
                .into(),
            )
            .await?;
        remove_date_from_cache(post_uri, cache_file).await?;
    }
    Ok(())
}

async fn bluesky_fetch_fav_dates(
    bsky_agent: &BskyAgent,
    cache_file_name: &str,
) -> Result<DatePostList> {
    let mut dates = (load_dates_from_cache(cache_file_name).await?).unwrap_or_default();
    // The Bluesky API does not provide a way to get all favorites of an actor
    // efficiently. It returns a cursor to fetch the next page of potential
    // favorites, but will return lots of empty pages. We stop after 100
    // requests and save the cursor for the next run.
    let cursor_file = &cache_file("bluesky_fav_cursor_cache.json");
    let mut cursor = if let Ok(json) = fs::read_to_string(cursor_file).await {
        match serde_json::from_str(&json)? {
            Some(cursor) => Some(cursor),
            None => {
                if !dates.is_empty() {
                    // Return early: the stored cursor is None which means
                    // all old favs have been fetched.
                    return Ok(dates);
                }
                None
            }
        }
    } else {
        None
    };

    let actor: AtIdentifier = bsky_agent.get_session().await.unwrap().did.clone().into();
    let mut counter = 0;

    loop {
        println!(
            "Fetching Bluesky favorites older than {}",
            cursor.as_ref().unwrap_or(&"now".to_string())
        );
        // Try to fetch as many posts as possible at once, Bluesky API docs say
        // that is 100.
        let feed = match bsky_agent
            .api
            .app
            .bsky
            .feed
            .get_actor_likes(
                bsky_sdk::api::app::bsky::feed::get_actor_likes::ParametersData {
                    actor: actor.clone(),
                    cursor: cursor.clone(),
                    limit: Some(LimitedNonZeroU8::try_from(100).unwrap()),
                }
                .into(),
            )
            .await
        {
            Ok(posts) => posts,
            Err(e) => {
                eprintln!("Error fetching favorites from Bluesky: {e:#?}");
                break;
            }
        };

        for post in &feed.feed {
            let record = bsky_sdk::api::app::bsky::feed::post::RecordData::try_from_unknown(
                post.post.record.clone(),
            )
            .expect("Failed to parse Bluesky post record for favorites");
            dates.insert(post.post.uri.clone(), (*record.created_at.as_ref()).into());
        }
        if feed.cursor.is_none() || feed.cursor == cursor {
            // The cursor did not change, we are at the beginning of the feed.
            // Reset the cursor and stop.
            cursor = None;
            break;
        }
        cursor = feed.cursor.clone();
        counter += 1;
        if counter >= 100 {
            break;
        }
    }

    save_dates_to_cache(cache_file_name, &dates).await?;
    let json = serde_json::to_string_pretty(&cursor)?;
    fs::write(cursor_file, json.as_bytes()).await?;

    Ok(dates)
}
