use anyhow::Result;
use chrono::prelude::*;
use chrono::Duration;
use megalodon::error::Kind;
use megalodon::megalodon::GetFavouritesInputOptions;
use megalodon::Megalodon;
use std::collections::BTreeMap;

use crate::cache_file;
use crate::config::*;

// Delete old favourites of this account that are older than 90 days.
pub async fn mastodon_delete_older_favs(
    mastodon: &Box<dyn Megalodon + Send + Sync>,
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
                                    remove_date_from_cache(&toot_id, cache_file).await?;
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
    mastodon: &Box<dyn Megalodon + Send + Sync>,
    cache_file: &str,
) -> Result<DatePostList> {
    match load_dates_from_cache(cache_file).await? {
        Some(dates) => Ok(dates),
        None => mastodon_fetch_fav_dates(mastodon, cache_file).await,
    }
}

async fn mastodon_fetch_fav_dates(
    mastodon: &Box<dyn Megalodon + Send + Sync>,
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
