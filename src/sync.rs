use anyhow::Result;
use bsky_sdk::api::app::bsky::feed::defs::FeedViewPostData;
use bsky_sdk::api::types::Object;
use megalodon::entities::Status;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use unicode_segmentation::UnicodeSegmentation;

// Represents new status updates that should be posted to Twitter (tweets) and
// Mastodon (toots).
#[derive(Debug, Clone)]
pub struct StatusUpdates {
    pub bsky_posts: Vec<NewStatus>,
    pub toots: Vec<NewStatus>,
}

impl StatusUpdates {
    /// Reverses the order of statuses in place.
    pub fn reverse_order(&mut self) {
        self.bsky_posts.reverse();
        self.toots.reverse();
    }
}

// A new status for posting. Optionally has links to media (images) that should
// be attached.
#[derive(Debug, Clone)]
pub struct NewStatus {
    pub text: String,
    pub attachments: Vec<NewMedia>,
    // A list of further statuses that are new replies to this new status. Used
    // to sync threads.
    pub replies: Vec<NewStatus>,
    // This new status could be part of a thread, post it in reply to an
    // existing already synced status.
    pub in_reply_to_id: Option<String>,
    // The original post ID on the source status.
    pub original_id: String,
}

#[derive(Debug, Clone)]
pub struct NewMedia {
    pub attachment_url: String,
    pub alt_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub sync_reblogs: bool,
    pub sync_reskeets: bool,
    pub sync_hashtag_twitter: Option<String>,
    pub sync_hashtag_mastodon: Option<String>,
}

/// This is the main synchronization function that can be tested without
/// external API calls.
///
/// The ordering of the statuses in both list parameters is expected to be from
/// newest to oldest. That is also the ordering returned by the Twitter and
/// Mastodon APIs for their timelines, they start with newest posts first.
///
/// The returned data structure contains new posts that are not synchronized yet
/// and should be posted on both Twitter and Mastodon. They are ordered in
/// reverse so that older statuses are posted first if there are multiple
/// statuses to synchronize.
pub fn determine_posts(
    mastodon_statuses: &[Status],
    bsky_statuses: &[Object<FeedViewPostData>],
    options: &SyncOptions,
) -> StatusUpdates {
    let mut updates = StatusUpdates {
        bsky_posts: Vec::new(),
        toots: Vec::new(),
    };
    'bsky: for post in bsky_statuses {
        // Skip replies, they are handled in determine_thread_replies().
        if let Some(_reply) = &post.reply {
            continue;
        }

        if !options.sync_reskeets {
            if let Some(_reskeet) = &post.post.viewer {
                if let Some(_repost) = &_reskeet.repost {
                    // Skip retweets when sync_retweets is disabled
                    continue;
                }
            }
        }

        for toot in mastodon_statuses {
            // Skip replies because we don't want to sync them here.
            if let Some(_id) = &toot.in_reply_to_id {
                continue;
            }
            // If the tweet already exists we can stop here and know that we are
            // synced.
            if toot_and_post_are_equal(toot, post) {
                break 'bsky;
            }
        }

        // The tweet is not on Mastodon yet, check if we should post it.
        // Fetch the tweet text into a String object
        let decoded_tweet = bsky_post_unshorten_decode(post);

        // Check if hashtag filtering is enabled and if the tweet matches.
        if let Some(sync_hashtag) = &options.sync_hashtag_twitter {
            if !sync_hashtag.is_empty() && !decoded_tweet.contains(sync_hashtag) {
                // Skip if a sync hashtag is set and the string doesn't match.
                continue;
            }
        }

        updates.toots.push(NewStatus {
            text: decoded_tweet,
            attachments: bsky_get_attachments(post),
            replies: Vec::new(),
            in_reply_to_id: None,
            original_id: post.post.uri.clone(),
        });
    }

    'toots: for toot in mastodon_statuses {
        // Skip replies, they are handled in determine_thread_replies().
        if let Some(_id) = &toot.in_reply_to_id {
            continue;
        }

        if toot.reblog.is_some() && !options.sync_reblogs {
            // Skip reblogs when sync_reblogs is disabled
            continue;
        }
        let fulltext = mastodon_toot_get_text(toot);
        // If this is a reblog/boost then take the URL to the original toot.
        let post = match &toot.reblog {
            None => bsky_post_shorten(&fulltext, &toot.url),
            Some(reblog) => bsky_post_shorten(&fulltext, &reblog.url),
        };
        // Skip direct toots to other Mastodon users, even if they are public.
        if post.starts_with('@') {
            continue;
        }

        for tweet in bsky_statuses {
            // If the toot already exists we can stop here and know that we are
            // synced.
            if toot_and_post_are_equal(toot, tweet) {
                break 'toots;
            }
        }

        // The toot is not on Twitter yet, check if we should post it.
        // Check if hashtag filtering is enabled and if the tweet matches.
        if let Some(sync_hashtag) = &options.sync_hashtag_mastodon {
            if !sync_hashtag.is_empty() && !fulltext.contains(sync_hashtag) {
                // Skip if a sync hashtag is set and the string doesn't match.
                continue;
            }
        }

        updates.bsky_posts.push(NewStatus {
            text: post,
            attachments: toot_get_attachments(toot),
            replies: Vec::new(),
            in_reply_to_id: None,
            original_id: toot
                .id
                .parse()
                .unwrap_or_else(|_| panic!("Mastodon status ID is not u64: {}", toot.id)),
        });
    }

    //determine_thread_replies(mastodon_statuses, bsky_statuses, options, &mut updates);

    // Older posts should come first to preserve the ordering of posts to
    // synchronize.
    updates.reverse_order();
    updates
}

/*fn bsky_post_is_reply(post: &Object<FeedViewPostData>) -> bool {
    if let Some(_reskeet) = &post.post.viewer {
        if let Some(_repost) = _reskeet.repost {
            // Skip retweets when sync_retweets is disabled
            continue;
        }
    }
}*/

// Returns true if a Mastodon toot and a Twitter tweet are considered equal.
pub fn toot_and_post_are_equal(toot: &Status, bsky_post: &Object<FeedViewPostData>) -> bool {
    // Make sure the structure is the same: both must be replies or both must
    // not be replies.
    if (toot.in_reply_to_id.is_some() && bsky_post.reply.is_none())
        || (toot.in_reply_to_id.is_none() && bsky_post.reply.is_some())
    {
        return false;
    }

    // Strip markup from Mastodon toot and unify message for comparison.
    let toot_text = unify_post_content(mastodon_toot_get_text(toot));
    // Replace those ugly t.co URLs in the tweet text.
    let tweet_text = unify_post_content(bsky_post_unshorten_decode(bsky_post));

    if toot_text == tweet_text {
        return true;
    }
    // Mastodon allows up to 500 characters, so we might need to shorten the
    // toot. If this is a reblog/boost then take the URL to the original toot.
    let shortened_toot = unify_post_content(match &toot.reblog {
        None => bsky_post_shorten(&toot_text, &toot.url),
        Some(reblog) => bsky_post_shorten(&toot_text, &reblog.url),
    });

    if shortened_toot == tweet_text {
        return true;
    }

    false
}

// Unifies tweet text or toot text to a common format.
fn unify_post_content(content: String) -> String {
    let mut result = content.to_lowercase();
    // Remove http:// and https:// for comparing because Twitter sometimes adds
    // those randomly.
    result = result.replace("http://", "");
    result = result.replace("https://", "");

    // Support for old posts that started with "RT @\username:", we consider
    // them equal to "RT username:".
    if result.starts_with("rt @\\") {
        result = result.replacen("rt @\\", "rt ", 1);
    }
    // Support for old posts that started with "RT @username:", we consider them
    // equal to "RT username:".
    if result.starts_with("rt @") {
        result = result.replacen("rt @", "rt ", 1);
    }
    if result.starts_with("rt \\@") {
        result = result.replacen("rt \\@", "rt ", 1);
    }
    // Escape direct user mentions with \@.
    result = result.replace(" \\@", " @");
    result.replace(" @\\", " @")
}

// Replace t.co URLs and HTML entity decode &amp;.
// Directly include quote tweets in the text.
pub fn bsky_post_unshorten_decode(bsky_post: &Object<FeedViewPostData>) -> String {
    // We need to cleanup the tweet text while passing the tweet around.
    /*let mut tweet = bsky_post.clone();

    if let Some(retweet) = &tweet.retweeted_status {
        tweet.text = format!(
            "RT {}: {}",
            retweet
                .clone()
                .user
                .unwrap_or_else(|| panic!("Twitter user missing on retweet {}", retweet.id))
                .screen_name,
            tweet_get_text_with_quote(retweet)
        );
        tweet.entities.urls = retweet.entities.urls.clone();
        tweet.extended_entities = retweet.extended_entities.clone();
    }

    // Remove the last media link if there is one, we will upload attachments
    // directly to Mastodon.
    if let Some(media) = &tweet.extended_entities {
        for attachment in &media.media {
            tweet.text = tweet.text.replace(&attachment.url, "");
        }
    }
    tweet.text = tweet.text.trim().to_string();
    tweet.text = tweet_get_text_with_quote(&tweet);

    // Replace t.co URLs with the real links in tweets.
    for url in tweet.entities.urls {
        if let Some(expanded_url) = &url.expanded_url {
            tweet.text = tweet.text.replace(&url.url, expanded_url);
        }
    }

    // Escape direct user mentions with @\.
    tweet.text = tweet.text.replace(" @", " @\\").replace(" @\\\\", " @\\");

    // Twitterposts have HTML entities such as &amp;, we need to decode them.
    let decoded = html_escape::decode_html_entities(&tweet.text);*/

    toot_shorten(&bsky_post.post.record.text, &bsky_post.post.uri)
}

// If this is a quote tweet then include the original text.
fn bsky_post_get_text_with_quote(bsky_post: &Object<FeedViewPostData>) -> String {
    bsky_post.post.record.text
    /*match bsky_post.quoted_status {
            None => bsky_post.text.clone(),
            Some(ref quoted_tweet) => {
                // Prevent infinite quote tweets. We only want to include
                // the first level, so make sure that the original has any
                // quote tweet removed.
                let mut original = quoted_tweet.clone();
                original.quoted_status = None;
                let original_text = bsky_post_unshorten_decode(&original);
                let screen_name = &original
                    .user
                    .as_ref()
                    .unwrap_or_else(|| panic!("Twitter user missing on tweet {}", original.id))
                    .screen_name;
                let mut tweet_text = bsky_post.text.clone();

                // Remove quote link at the end of the tweet text.
                for url in &bsky_post.entities.urls {
                    if let Some(expanded_url) = &url.expanded_url {
                        if expanded_url
                            == &format!(
                                "https://twitter.com/{}/status/{}",
                                screen_name, quoted_tweet.id
                            )
                            || expanded_url
                                == &format!(
                                    "https://mobile.twitter.com/{}/status/{}",
                                    screen_name, quoted_tweet.id
                                )
                        {
                            tweet_text = tweet_text.replace(&url.url, "").trim().to_string();
                        }
                    }
                }

                format!(
                    "{tweet_text}

    QT {screen_name}: {original_text}"
                )
            }
        }*/
}

pub fn bsky_post_shorten(text: &str, toot_url: &Option<String>) -> String {
    let mut char_count = text.graphemes(true).count();
    let re = Regex::new(r"[^\s]+$").unwrap();
    let mut shortened = text.trim().to_string();
    let mut with_link = shortened.clone();

    // Bluesky should allow 280 characters, but their counting is unpredictable.
    // Use 40 characters less and hope it works ¯\_(ツ)_/¯
    while char_count > 240 {
        // Remove the last word.
        shortened = re.replace_all(&shortened, "").trim().to_string();
        if let Some(ref toot_url) = *toot_url {
            // Add a link to the toot that has the full text.
            with_link = shortened.clone() + "… " + toot_url;
        } else {
            with_link = shortened.clone();
        }
        let new_count = with_link.graphemes(true).count();
        char_count = new_count;
    }
    with_link
}

// Mastodon has a 500 character post limit. With embedded quote tweets and long
// links the content could get too long, shorten it to 500 characters.
fn toot_shorten(text: &str, post_uri: &str) -> String {
    let mut char_count = text.graphemes(true).count();
    let re = Regex::new(r"[^\s]+$").unwrap();
    let mut shortened = text.trim().to_string();
    let mut with_link = shortened.clone();

    // Hard-coding a limit of 500 here for now, could be configurable.
    while char_count > 500 {
        // Remove the last word.
        shortened = re.replace_all(&shortened, "").trim().to_string();
        // Add a link to the full length tweet.
        with_link = format!("{shortened}… https://twitter.com/twitter/status/{post_uri}");
        char_count = with_link.graphemes(true).count();
    }
    with_link
}

// Prefix boost toots with the author and strip HTML tags.
pub fn mastodon_toot_get_text(toot: &Status) -> String {
    let mut replaced = match toot.reblog {
        None => toot.content.clone(),
        Some(ref reblog) => format!("RT {}: {}", reblog.account.username, reblog.content),
    };
    replaced = replaced.replace("<br />", "\n");
    replaced = replaced.replace("<br>", "\n");
    replaced = replaced.replace("</p><p>", "\n\n");
    replaced = replaced.replace("<p>", "");
    replaced = replaced.replace("</p>", "");

    replaced = voca_rs::strip::strip_tags(&replaced);

    // Escape direct user mentions with @\.
    replaced = replaced.replace(" @", " @\\").replace(" @\\\\", " @\\");

    html_escape::decode_html_entities(&replaced).to_string()
}

// Ensure that sync posts have not been made before to prevent syncing loops.
// Use a cache file to temporarily store posts and compare them on the next
// invocation.
pub fn filter_posted_before(
    posts: StatusUpdates,
    post_cache: &HashSet<String>,
) -> Result<StatusUpdates> {
    // If there are no status updates then we don't need to check anything.
    if posts.toots.is_empty() && posts.bsky_posts.is_empty() {
        return Ok(posts);
    }

    let mut filtered_posts = StatusUpdates {
        bsky_posts: Vec::new(),
        toots: Vec::new(),
    };
    for tweet in posts.bsky_posts {
        if post_cache.contains(&tweet.text) {
            eprintln!(
                "Error: preventing double posting to Twitter: {}",
                tweet.text
            );
        } else {
            filtered_posts.bsky_posts.push(tweet.clone());
        }
    }
    for toot in posts.toots {
        if post_cache.contains(&toot.text) {
            eprintln!(
                "Error: preventing double posting to Mastodon: {}",
                toot.text
            );
        } else {
            filtered_posts.toots.push(toot.clone());
        }
    }

    Ok(filtered_posts)
}

// Read the JSON encoded cache file from disk or provide an empty default cache.
pub fn read_post_cache(cache_file: &str) -> HashSet<String> {
    match fs::read_to_string(cache_file) {
        Ok(json) => {
            match serde_json::from_str::<HashSet<String>>(&json) {
                Ok(cache) => {
                    // If the cache has more than 150 items already then empty it to not
                    // accumulate too many items and allow posting the same text at a
                    // later date.
                    if cache.len() > 150 {
                        HashSet::new()
                    } else {
                        cache
                    }
                }
                Err(_) => HashSet::new(),
            }
        }
        Err(_) => HashSet::new(),
    }
}

// Returns a list of direct links to attachments for download.
pub fn bsky_get_attachments(bsky_post: &Object<FeedViewPostData>) -> Vec<NewMedia> {
    let links = Vec::new();
    /*// Check if there are attachments directly on the tweet, otherwise try to
    // use attachments from retweets and quote tweets.
    let media = match &tweet.extended_entities {
        Some(media) => Some(media),
        None => {
            let mut retweet_media = None;
            if let Some(retweet) = &tweet.retweeted_status {
                if let Some(media) = &retweet.extended_entities {
                    retweet_media = Some(media);
                }
            } else if let Some(quote_tweet) = &tweet.quoted_status {
                if let Some(media) = &quote_tweet.extended_entities {
                    retweet_media = Some(media);
                }
            }
            retweet_media
        }
    };

    if let Some(media) = media {
        for attachment in &media.media {
            match &attachment.video_info {
                Some(video_info) => {
                    let mut bitrate = 0;
                    let mut media_url = "".to_string();
                    // Use the video variant with the highest bitrate.
                    for variant in &video_info.variants {
                        if let Some(video_bitrate) = variant.bitrate {
                            if video_bitrate >= bitrate {
                                bitrate = video_bitrate;
                                media_url = variant.url.clone();
                            }
                        }
                    }
                    links.push(NewMedia {
                        attachment_url: media_url,
                        alt_text: attachment.ext_alt_text.clone(),
                    });
                }
                None => {
                    links.push(NewMedia {
                        attachment_url: attachment.media_url_https.clone(),
                        alt_text: attachment.ext_alt_text.clone(),
                    });
                }
            }
        }
    }*/
    links
}

// Returns a list of direct links to attachments for download.
pub fn toot_get_attachments(toot: &Status) -> Vec<NewMedia> {
    let mut links = Vec::new();
    let mut attachments = &toot.media_attachments;
    // If there are no attachments check if this is a boost and if there might
    // be some attachments there.
    if attachments.is_empty() {
        if let Some(boost) = &toot.reblog {
            attachments = &boost.media_attachments;
        }
    }
    for attachment in attachments {
        links.push(NewMedia {
            attachment_url: attachment.url.clone(),
            // Twitter only allows a max length of 1,000 characters for alt
            // text, so we need to cut it off here.
            alt_text: truncate_option_string(attachment.description.clone(), 1_000),
        });
    }
    links
}

/// Truncates a given string to a maximum number of characters.
///
/// I could not find a Rust core function that does this? We don't care about
/// graphemes, please just cut off characters at a certain length. Copied from
/// https://stackoverflow.com/a/38461750/2000435
///
/// No, I will not install the substring crate just to get a substring, are you
/// kidding me????
fn truncate_option_string(stringy: Option<String>, max_chars: usize) -> Option<String> {
    match stringy {
        Some(string) => match string.char_indices().nth(max_chars) {
            None => Some(string),
            Some((idx, _)) => Some(string[..idx].to_string()),
        },
        None => None,
    }
}
