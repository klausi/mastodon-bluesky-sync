use anyhow::Context;
use anyhow::Result;
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
                if let megalodon::error::Error::OwnError(ref own_error) = error
                    && let Kind::HTTPStatusError = own_error.kind
                    && let Some(status) = own_error.status
                {
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
    if let Some(captures) = re.captures(link_header)
        && let Some(max_id) = captures.get(1)
        && let Ok(max_id) = max_id.as_str().parse::<u64>()
    {
        return Some(max_id);
    }
    None
}

// Delete old favorites (likes) of this account that are older than 90 days.
pub async fn bluesky_delete_older_favs(bsky_agent: &BskyAgent, dry_run: bool) -> Result<()> {
    // Cache like record URIs -> the like record's createdAt.
    let cache_file = &cache_file("bluesky_like_cache.json");
    let dates = bluesky_fetch_like_dates(bsky_agent, cache_file).await?;
    let three_months_ago = Utc::now() - Duration::days(90);
    let actor: AtIdentifier = bsky_agent.get_session().await.unwrap().did.clone().into();
    for (like_uri, date) in dates.iter().filter(|(_, date)| date < &&three_months_ago) {
        println!("Deleting Bluesky like (older than 90d) from {date}: {like_uri}");
        if dry_run {
            continue;
        }
        // Expected like URI format: at://<did>/app.bsky.feed.like/<rkey>
        let parts = like_uri
            .strip_prefix("at://")
            .with_context(|| format!("Invalid At URI prefix {like_uri} when deleting like"))?
            .splitn(3, '/')
            .collect::<Vec<_>>();
        if parts.len() != 3 {
            eprintln!("Skipping malformed like URI: {like_uri}");
            continue;
        }
        let collection = parts[1];
        if collection != "app.bsky.feed.like" {
            // Legacy cache entry from old implementation referencing a post URI -> just drop it.
            eprintln!("Skipping non-like cached entry: {like_uri}");
            remove_date_from_cache(like_uri, cache_file).await?;
            continue;
        }
        let rkey = match parts[2].parse::<RecordKey>() {
            Ok(rkey) => rkey,
            Err(e) => {
                eprintln!("Invalid like rkey in {like_uri}: {e}");
                remove_date_from_cache(like_uri, cache_file).await?;
                continue;
            }
        };
        if let Err(e) = bsky_agent
            .api
            .com
            .atproto
            .repo
            .delete_record(
                bsky_sdk::api::com::atproto::repo::delete_record::InputData {
                    collection: Nsid::new("app.bsky.feed.like".to_string()).unwrap(),
                    repo: actor.clone(),
                    rkey,
                    swap_commit: None,
                    swap_record: None,
                }
                .into(),
            )
            .await
        {
            // If the record is already gone treat it as success.
            eprintln!("Error deleting like {like_uri}: {e:#?}");
            // We still remove it from cache to avoid trying again forever; adjust if you prefer retry.
        }
        remove_date_from_cache(like_uri, cache_file).await?;
    }
    Ok(())
}

// Fetch (or extend cached) like record creation dates by listing our own like records.
async fn bluesky_fetch_like_dates(
    bsky_agent: &BskyAgent,
    cache_file_name: &str,
) -> Result<DatePostList> {
    // Load existing cache (may contain legacy post URIs which we'll ignore on delete).
    let mut dates = (load_dates_from_cache(cache_file_name).await?).unwrap_or_default();

    // Cursor cache for incremental listing of like records.
    let cursor_file = &cache_file("bluesky_like_cursor_cache.json");
    let mut cursor: Option<String> = if let Ok(json) = fs::read_to_string(cursor_file).await {
        serde_json::from_str(&json).unwrap_or(None)
    } else {
        None
    };

    if !dates.is_empty() && cursor.is_none() {
        // We already have a full cache and don't need to fetch likes.
        return Ok(dates);
    }

    let actor: AtIdentifier = bsky_agent.get_session().await.unwrap().did.clone().into();
    let mut counter = 0usize;

    loop {
        println!(
            "Listing Bluesky like records starting from {}",
            cursor.as_deref().unwrap_or("beginning")
        );
        // Use list_records on our repo for the like collection.
        let response = match bsky_agent
            .api
            .com
            .atproto
            .repo
            .list_records(
                bsky_sdk::api::com::atproto::repo::list_records::ParametersData {
                    repo: actor.clone(),
                    collection: Nsid::new("app.bsky.feed.like".to_string()).unwrap(),
                    cursor: cursor.clone(),
                    limit: Some(LimitedNonZeroU8::try_from(100).unwrap()),
                    reverse: None,
                }
                .into(),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error listing like records: {e:#?}");
                break; // Keep what we have so far.
            }
        };

        for rec in &response.records {
            // Parse like record value to extract its createdAt (time we liked the post).
            let like_record = bsky_sdk::api::app::bsky::feed::like::RecordData::try_from_unknown(
                rec.value.clone(),
            )
            .expect("Failed to parse like record");
            dates.insert(rec.uri.clone(), (*like_record.created_at.as_ref()).into());
        }

        let new_cursor = response.cursor.clone();
        if new_cursor.is_none() || new_cursor == cursor {
            // Completed traversal.
            cursor = None;
            break;
        }
        cursor = new_cursor;
        counter += 1;
        if counter >= 100 {
            // Throttle to avoid huge traversals in one run.
            break;
        }
    }

    save_dates_to_cache(cache_file_name, &dates).await?;
    let json = serde_json::to_string_pretty(&cursor)?;
    fs::write(cursor_file, json.as_bytes()).await?;

    Ok(dates)
}
