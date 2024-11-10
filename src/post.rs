use crate::sync::NewStatus;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use megalodon::megalodon::PostStatusOutput;
use megalodon::megalodon::UploadMediaInputOptions;
use megalodon::Megalodon;
use megalodon::{
    entities::{self, StatusVisibility},
    error,
    megalodon::PostStatusInputOptions,
};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tempfile::tempdir;
use tokio::time::sleep;

/// Send new status with any given replies to Mastodon.
pub async fn post_to_mastodon(
    mastodon: &Box<dyn Megalodon + Send + Sync>,
    toot: &NewStatus,
    dry_run: bool,
) -> Result<()> {
    if let Some(reply_to) = &toot.in_reply_to_id {
        println!(
            "Posting thread reply for {} to Mastodon: {}",
            reply_to, toot.text
        );
    } else {
        println!("Posting to Mastodon: {}", toot.text);
    }
    let mut status_id = "".to_string();
    if !dry_run {
        status_id = send_single_post_to_mastodon(mastodon, toot).await?;
    }

    // Recursion does not work well with async functions, so we use iteration
    // here instead.
    let mut replies = Vec::new();
    for reply in &toot.replies {
        replies.push((status_id.clone(), reply));
    }

    while !replies.is_empty() {
        let (parent_id, reply) = replies.remove(0);
        let mut new_reply = reply.clone();
        // Set the new ID of the parent status to reply to.
        new_reply.in_reply_to_id = Some(parent_id.clone());

        println!(
            "Posting thread reply for {} to Mastodon: {}",
            &parent_id, reply.text
        );
        let mut parent_status_id = "".to_string();
        if !dry_run {
            parent_status_id = send_single_post_to_mastodon(mastodon, &new_reply).await?;
        }
        for remaining_reply in &reply.replies {
            replies.push((parent_status_id.clone(), remaining_reply));
        }
    }

    Ok(())
}

/// Sends the given new status to Mastodon.
async fn send_single_post_to_mastodon(
    mastodon: &Box<dyn Megalodon + Send + Sync>,
    toot: &NewStatus,
) -> Result<String> {
    let mut media_ids = Vec::new();
    // Temporary directory where we will download any file attachments to.
    let temp_dir = tempdir()?;
    // Post attachments first, if there are any.
    for attachment in &toot.attachments {
        // Because we use async for egg-mode we also need to use reqwest in
        // async mode. Otherwise we get double async executor errors.
        let response = reqwest::get(&attachment.attachment_url)
            .await
            .context(format!(
                "Failed downloading attachment {}",
                attachment.attachment_url
            ))?;
        let file_name = match Path::new(response.url().path()).file_name() {
            Some(f) => f,
            None => bail!(
                "Failed to create file name from attachment {}",
                attachment.attachment_url
            ),
        };

        let path = temp_dir.path().join(file_name);
        let string_path = path.to_string_lossy().into_owned();

        let mut file = File::create(path)?;
        file.write_all(&response.bytes().await?)?;

        let upload = match &attachment.alt_text {
            None => mastodon.upload_media(string_path, None).await?,
            Some(description) => {
                mastodon
                    .upload_media(
                        string_path,
                        Some(&UploadMediaInputOptions {
                            description: Some(description.clone()),
                            focus: None,
                        }),
                    )
                    .await?
            }
        }
        .json();

        match upload {
            entities::UploadMedia::Attachment(attachment) => {
                media_ids.push(attachment.id);
            }
            entities::UploadMedia::AsyncAttachment(async_attachment) => {
                let uploaded = mastodon_wait_until_uploaded(mastodon, &async_attachment.id).await?;
                media_ids.push(uploaded.id);
            }
        }
    }

    let status = mastodon
        .post_status(
            toot.text.clone(),
            Some(&PostStatusInputOptions {
                media_ids: Some(media_ids),
                sensitive: Some(false),
                visibility: Some(StatusVisibility::Public),
                ..Default::default()
            }),
        )
        .await?
        .json();

    match status {
        PostStatusOutput::Status(status) => Ok(status.id),
        PostStatusOutput::ScheduledStatus(scheduled_status) => bail!(
            "Scheduled status returned instead of normal Status: {:?}",
            scheduled_status
        ),
    }
}

async fn mastodon_wait_until_uploaded(
    client: &Box<dyn Megalodon + Send + Sync>,
    id: &str,
) -> Result<entities::Attachment, error::Error> {
    loop {
        let res = client.get_media(id.to_string()).await;
        return match res {
            Ok(res) => Ok(res.json()),
            Err(err) => match err {
                error::Error::OwnError(ref own_err) => match own_err.kind {
                    error::Kind::HTTPPartialContentError => {
                        sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                    _ => Err(err),
                },
                _ => Err(err),
            },
        };
    }
}

/*/// Send a new status update to Twitter, including thread replies and
/// attachments.
pub async fn post_to_twitter(token: &Token, tweet: &NewStatus, dry_run: bool) -> Result<()> {
    if let Some(reply_to) = tweet.in_reply_to_id {
        println!(
            "Posting thread reply for {} to Twitter: {}",
            reply_to, tweet.text
        );
    } else {
        println!("Posting to Twitter: {}", tweet.text);
    }
    let mut status_id = 0;
    if !dry_run {
        status_id = send_single_post_to_twitter(token, tweet).await?;
    }

    // Recursion does not work well with async functions, so we use iteration
    // here instead.
    let mut replies = Vec::new();
    for reply in &tweet.replies {
        replies.push((status_id, reply));
    }

    while !replies.is_empty() {
        let (parent_id, reply) = replies.remove(0);
        let mut new_reply = reply.clone();
        // Set the new ID of the parent status to reply to.
        new_reply.in_reply_to_id = Some(parent_id);

        println!(
            "Posting thread reply for {} to Twitter: {}",
            parent_id, reply.text
        );
        let mut parent_status_id = 0;
        if !dry_run {
            parent_status_id = send_single_post_to_twitter(token, &new_reply).await?;
        }
        for remaining_reply in &reply.replies {
            replies.push((parent_status_id, remaining_reply));
        }
    }

    Ok(())
}

/// Sends the given new status to Twitter.
async fn send_single_post_to_twitter(token: &Token, tweet: &NewStatus) -> Result<u64> {
    let mut draft = DraftTweet::new(tweet.text.clone());
    'attachments: for attachment in &tweet.attachments {
        let response = reqwest::get(&attachment.attachment_url).await?;
        let media_type = response
            .headers()
            .get(CONTENT_TYPE)
            .ok_or_else(|| format_err!("Missing content-type on response"))?
            .to_str()?
            .parse::<mime::Mime>()?;

        let bytes = response.bytes().await?;
        let mut media_handle = upload_media(&bytes, &media_type, token).await?;

        // Now we need to wait and check until the media is ready.
        loop {
            let wait_seconds = match media_handle.progress {
                Some(progress) => match progress {
                    Pending(seconds) | InProgress(seconds) => seconds,
                    Failed(error) => {
                        if error.code == 3 {
                            warn!(
                                "Skipping unsupported media attachment {}, because of {}",
                                attachment.attachment_url, error
                            );
                            continue 'attachments;
                        }
                        return Err(format_err!(
                            "Twitter media upload of {} failed: {}",
                            attachment.attachment_url,
                            error
                        ));
                    }
                    Success => 0,
                },
                // If there is no progress assume that processing is done.
                None => 0,
            };

            if wait_seconds > 0 {
                sleep(Duration::from_secs(wait_seconds)).await;
                media_handle = egg_mode::media::get_status(media_handle.id, token).await?;
            } else {
                break;
            }
        }

        draft.add_media(media_handle.id.clone());
        if let Some(alt_text) = &attachment.alt_text {
            set_metadata(&media_handle.id, alt_text, token).await?;
        }
    }

    let created_tweet = if let Some(parent_id) = tweet.in_reply_to_id {
        draft.in_reply_to(parent_id).send(token).await?
    } else {
        draft.send(token).await?
    };

    Ok(created_tweet.id)
}*/
