use anyhow::Result;
use bsky_sdk::api::types::LimitedNonZeroU8;
use bsky_sdk::api::types::TryFromUnknown;
use bsky_sdk::BskyAgent;
use chrono::prelude::*;
use chrono::Duration;
use std::collections::BTreeMap;
use tokio::fs;
use tokio::fs::remove_file;

use crate::cache_file;

type DatePostMap = BTreeMap<DateTime<Utc>, String>;

// Delete old posts of this account that are older than 90 days.
pub async fn bluesky_delete_older_posts(bsky_agent: &BskyAgent, dry_run: bool) -> Result<()> {
    // In order not to fetch old posts every time keep them in a cache file
    // keyed by their dates.
    let cache_file = &cache_file("bluesky_cache.json");
    let dates = bluesky_load_post_dates(bsky_agent, cache_file).await?;
    let mut remove_dates = Vec::new();
    let three_months_ago = Utc::now() - Duration::days(90);
    for (date, post_uri) in dates.range(..three_months_ago) {
        println!("Deleting Bluesky post from {date}: {post_uri}");
        // Do nothing on a dry run, just print what would be done.
        if dry_run {
            continue;
        }
        remove_dates.push(date);
        let delete_result = bsky_agent.delete_record(post_uri).await;
        // The status could have been deleted already by the user, ignore API
        // errors in that case.
        if let Err(e) = delete_result {
            eprintln!("Failed to delete post {post_uri} (maybe the post was already deleted): {e}");
        }
    }
    remove_dates_from_cache(remove_dates, &dates, cache_file).await
}

async fn bluesky_load_post_dates(bsky_agent: &BskyAgent, cache_file: &str) -> Result<DatePostMap> {
    match load_dates_from_cache(cache_file).await? {
        Some(dates) => Ok(dates),
        None => bluesky_fetch_post_dates(bsky_agent, cache_file).await,
    }
}

async fn bluesky_fetch_post_dates(bsky_agent: &BskyAgent, cache_file: &str) -> Result<DatePostMap> {
    let mut dates = BTreeMap::new();
    let mut cursor = None;
    loop {
        // Try to fetch as many posts as possible at once, Bluesky API docs say
        // that is 100.
        let feed = match bsky_agent
            .api
            .app
            .bsky
            .feed
            .get_author_feed(
                bsky_sdk::api::app::bsky::feed::get_author_feed::ParametersData {
                    actor: bsky_agent.get_session().await.unwrap().did.clone().into(),
                    cursor: cursor,
                    filter: None,
                    include_pins: None,
                    limit: Some(LimitedNonZeroU8::try_from(100).unwrap()),
                }
                .into(),
            )
            .await
        {
            Ok(posts) => posts,
            Err(e) => {
                eprintln!("Error fetching posts from Bluesky: {e:#?}");
                break;
            }
        };

        for post in &feed.feed {
            let record = bsky_sdk::api::app::bsky::feed::post::RecordData::try_from_unknown(
                post.post.record.clone(),
            )
            .expect("Failed to parse Bluesky post record");

            // Check if this post is a repost.
            if let Some(viewer) = &post.post.viewer {
                if let Some(repost) = &viewer.repost {
                    dates.insert(
                        record.created_at.as_ref().clone().into(),
                        repost.to_string(),
                    );
                    continue;
                }
            }
            dates.insert(
                record.created_at.as_ref().clone().into(),
                post.post.uri.clone(),
            );
        }
        if feed.cursor.is_none() {
            break;
        }
        cursor = feed.cursor.clone();
    }

    save_dates_to_cache(cache_file, &dates).await?;

    Ok(dates)
}

async fn load_dates_from_cache(cache_file: &str) -> Result<Option<DatePostMap>> {
    if let Ok(json) = fs::read_to_string(cache_file).await {
        let cache = serde_json::from_str(&json)?;
        Ok(Some(cache))
    } else {
        Ok(None)
    }
}

async fn save_dates_to_cache(cache_file: &str, dates: &DatePostMap) -> Result<()> {
    let json = serde_json::to_string_pretty(&dates)?;
    fs::write(cache_file, json.as_bytes()).await?;
    Ok(())
}

// Delete a list of dates from the given cache of dates and write the cache to
// disk if necessary.
async fn remove_dates_from_cache(
    remove_dates: Vec<&DateTime<Utc>>,
    cached_dates: &DatePostMap,
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
