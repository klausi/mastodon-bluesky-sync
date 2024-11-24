use anyhow::Result;
use bsky_sdk::api::types::LimitedNonZeroU8;
use bsky_sdk::api::types::TryFromUnknown;
use bsky_sdk::BskyAgent;
use chrono::prelude::*;
use chrono::Duration;
use std::collections::BTreeMap;

use crate::cache_file;
use crate::load_dates_from_cache;
use crate::remove_date_from_cache;
use crate::save_dates_to_cache;
use crate::DatePostMap;

// Delete old posts of this account that are older than 90 days.
pub async fn bluesky_delete_older_posts(bsky_agent: &BskyAgent, dry_run: bool) -> Result<()> {
    // In order not to fetch old posts every time keep them in a cache file
    // keyed by their dates.
    let cache_file = &cache_file("bluesky_cache.json");
    let dates = bluesky_load_post_dates(bsky_agent, cache_file).await?;
    let three_months_ago = Utc::now() - Duration::days(90);
    for (date, post_uri) in dates.range(..three_months_ago) {
        println!("Deleting Bluesky post from {date}: {post_uri}");
        // Do nothing on a dry run, just print what would be done.
        if dry_run {
            continue;
        }
        // No error handling needed here for non existing posts, the Bluesky API
        // returns success even if the post does not exist.
        bsky_agent.delete_record(post_uri).await?;
        remove_date_from_cache(date, cache_file).await?;
    }
    Ok(())
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
