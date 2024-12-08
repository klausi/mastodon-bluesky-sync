use crate::bluesky_richtext::get_rich_text;
use crate::bluesky_video::bluesky_upload_video;
use crate::sync::NewStatus;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use bsky_sdk::BskyAgent;
use image_compressor::compressor::Compressor;
use image_compressor::Factor;
use megalodon::megalodon::PostStatusOutput;
use megalodon::megalodon::UploadMediaInputOptions;
use megalodon::Megalodon;
use megalodon::{
    entities::{self, StatusVisibility},
    error,
    megalodon::PostStatusInputOptions,
};
use serde_json::to_string;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tempfile::tempdir;
use tempfile::NamedTempFile;
use tokio::fs::metadata;
use tokio::time::sleep;

/// Send new status with any given replies to Mastodon.
pub async fn post_to_mastodon(
    mastodon: &(dyn Megalodon + Send + Sync),
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
    mastodon: &(dyn Megalodon + Send + Sync),
    toot: &NewStatus,
) -> Result<String> {
    let mut media_ids = Vec::new();
    // Temporary directory where we will download any file attachments to.
    let temp_dir = tempdir()?;
    // Post attachments first, if there are any.
    for attachment in &toot.attachments {
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
    client: &(dyn Megalodon + Send + Sync),
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

/// Send a new status update to Bluesky, including thread replies and
/// attachments.
pub async fn post_to_bluesky(
    bsky_agent: &BskyAgent,
    post: &NewStatus,
    dry_run: bool,
) -> Result<()> {
    if let Some(reply_to) = &post.in_reply_to_id {
        println!(
            "Posting thread reply for {} to Bluesky: {}",
            reply_to, post.text
        );
    } else {
        println!("Posting to Bluesky: {}", post.text);
    }
    let mut status_id = "".to_string();
    if !dry_run {
        status_id = send_single_post_to_bluesky(bsky_agent, post).await?;
    }

    // Recursion does not work well with async functions, so we use iteration
    // here instead.
    let mut replies = Vec::new();
    for reply in &post.replies {
        replies.push((status_id.clone(), reply));
    }

    while !replies.is_empty() {
        let (parent_id, reply) = replies.remove(0);
        let mut new_reply = reply.clone();
        // Set the new ID of the parent status to reply to.
        new_reply.in_reply_to_id = Some(parent_id.clone());

        println!(
            "Posting thread reply for {} to Bluesky: {}",
            &parent_id, reply.text
        );
        let mut parent_status_id = "".to_string();
        if !dry_run {
            parent_status_id = send_single_post_to_bluesky(bsky_agent, &new_reply).await?;
        }
        for remaining_reply in &reply.replies {
            replies.push((parent_status_id.clone(), remaining_reply));
        }
    }

    Ok(())
}

/// Sends the given new status to Bluesky.
async fn send_single_post_to_bluesky(bsky_agent: &BskyAgent, post: &NewStatus) -> Result<String> {
    let mut images = Vec::new();
    let mut video = None;
    for attachment in &post.attachments {
        let response = reqwest::get(&attachment.attachment_url)
            .await
            .context(format!(
                "Failed downloading attachment {}",
                attachment.attachment_url
            ))?;
        let content_type = response
            .headers()
            .get("content-type")
            .context(format!(
                "Failed getting content type of {}",
                &attachment.attachment_url
            ))?
            .to_str()
            .context(format!(
                "Failed converting content type of {} to string",
                &attachment.attachment_url
            ))?
            .to_string();
        let bytes = response.bytes().await?;

        if content_type.starts_with("image/") {
            let attachment_bytes =
                resize_image_if_needed(&bytes, &attachment.attachment_url).await?;

            let output = bsky_agent
                .api
                .com
                .atproto
                .repo
                .upload_blob(attachment_bytes)
                .await
                .context(format!(
                    "Failed uploading image to Bluesky {}",
                    attachment.attachment_url
                ))?;
            images.push(
                bsky_sdk::api::app::bsky::embed::images::ImageData {
                    alt: attachment.alt_text.clone().unwrap_or_default(),
                    aspect_ratio: None,
                    image: output.data.blob,
                }
                .into(),
            );
        } else if content_type.starts_with("video/") {
            let blob =
                bluesky_upload_video(bsky_agent, &attachment.attachment_url, bytes.into()).await?;
            video = Some(bsky_sdk::api::app::bsky::embed::video::MainData {
                alt: attachment.alt_text.clone(),
                aspect_ratio: None,
                captions: None,
                video: blob,
            });
        }
    }
    // If there is a video then prefer that as embed, otherwise use images.
    let embed = match video {
        None => Some(bsky_sdk::api::types::Union::Refs(
            bsky_sdk::api::app::bsky::feed::post::RecordEmbedRefs::AppBskyEmbedImagesMain(
                Box::new(bsky_sdk::api::app::bsky::embed::images::MainData { images }.into()),
            ),
        )),
        Some(video) => Some(bsky_sdk::api::types::Union::Refs(
            bsky_sdk::api::app::bsky::feed::post::RecordEmbedRefs::AppBskyEmbedVideoMain(Box::new(
                video.into(),
            )),
        )),
    };

    let rt = get_rich_text(&post.text);
    let record = bsky_agent
        .create_record(bsky_sdk::api::app::bsky::feed::post::RecordData {
            created_at: bsky_sdk::api::types::string::Datetime::now(),
            embed,
            entities: None,
            facets: rt.facets,
            labels: None,
            langs: None,
            reply: None,
            tags: None,
            text: rt.text,
        })
        .await
        .context(format!("Failed posting to Bluesky {}", post.text))?;

    Ok(to_string(&record.cid)?)
}

async fn resize_image_if_needed(download_bytes: &[u8], url: &str) -> Result<Vec<u8>> {
    // Check that the image is not larger than 1MB.
    let size = download_bytes.len();
    if size > 1_000_000 {
        let mut source_file = NamedTempFile::new()?;
        source_file.write_all(download_bytes)?;
        // Try with 100% quality first, then decrease by 10% until we
        // get less than 1MB.
        let mut quality = 100.;
        loop {
            let dest_dir = tempdir()?;
            let mut compressor = Compressor::new(source_file.path(), dest_dir.path());
            compressor.set_factor(Factor::new(quality, 1.0));
            // Dyn errors are weird, can't throw them with `?`.`
            let compressed = match compressor.compress_to_jpg() {
                Ok(compressed) => compressed,
                Err(e) => {
                    bail!("Failed compressing image {url} to less than 1MB: {e}");
                }
            };
            let new_size = metadata(&compressed).await?.len();
            if new_size < 1_000_000 {
                let mut compressed_file = File::open(compressed)?;
                let mut compressed_bytes = Vec::new();
                compressed_file.read_to_end(&mut compressed_bytes)?;
                return Ok(compressed_bytes);
            }
            quality -= 10.;
            if quality < 0.1 {
                bail!("Could not compress image {url} to less than 1MB");
            }
        }
    }
    Ok(download_bytes.to_vec())
}
