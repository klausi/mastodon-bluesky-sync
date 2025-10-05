use crate::BskyAgent;
use crate::NewMedia;
use crate::bluesky_richtext::get_rich_text;
use crate::bluesky_video::bluesky_upload_video;
use crate::sync::NewStatus;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use bsky_sdk::api::app::bsky::feed::post::RecordEmbedRefs;
use bsky_sdk::api::types::BlobRef;
use image_compressor::Factor;
use image_compressor::compressor::Compressor;
use megalodon::Megalodon;
use megalodon::megalodon::PostStatusOutput;
use megalodon::megalodon::UploadMediaInputOptions;
use megalodon::{
    entities::{self, StatusVisibility},
    error,
    megalodon::PostStatusInputOptions,
};
use serde_json::to_string;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tempfile::NamedTempFile;
use tempfile::tempdir;
use tokio::fs::File;
use tokio::fs::metadata;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
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
    // Post attachments first, if there are any.
    let mut media_ids = Vec::new();
    if let Some(video_stream) = &toot.video_stream {
        let media_id = mastodon_upload_video_stream(mastodon, video_stream).await?;
        media_ids.push(media_id);
    }
    // Temporary directory where we will download any file attachments to.
    let temp_dir = tempdir()?;
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

        let mut file = File::create(path).await?;
        file.write_all(&response.bytes().await?).await?;

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

// Download a Bluesky video stream, convert it with ffmpeg and upload it to
// Mastodon. Returns the media ID of the uploaded video.
async fn mastodon_upload_video_stream(
    mastodon: &(dyn Megalodon + Send + Sync),
    stream_url: &str,
) -> Result<String> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("video.mp4");
    let command = Command::new("ffmpeg")
        .arg("-i")
        .arg(stream_url)
        .arg("-acodec")
        .arg("copy")
        .arg("-bsf:a")
        .arg("aac_adtstoasc")
        .arg("-vcodec")
        .arg("copy")
        .arg(path.to_string_lossy().to_string())
        .output()
        .context(format!(
            "Failed to execute ffmpeg for video stream {stream_url}"
        ))?;
    if !command.status.success() {
        bail!(
            "ffmpeg error for {stream_url}: {}",
            String::from_utf8_lossy(&command.stderr)
        );
    }

    let upload = mastodon
        .upload_media(path.to_string_lossy().to_string(), None)
        .await?
        .json();

    Ok(match upload {
        entities::UploadMedia::Attachment(attachment) => attachment.id,
        entities::UploadMedia::AsyncAttachment(async_attachment) => {
            let uploaded = mastodon_wait_until_uploaded(mastodon, &async_attachment.id).await?;
            uploaded.id
        }
    })
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
    let mut embed = None;
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
            images.push(
                bsky_sdk::api::app::bsky::embed::images::ImageData {
                    alt: attachment.alt_text.clone().unwrap_or_default(),
                    aspect_ratio: None,
                    image: bluesky_upload_image(&bytes, &attachment.attachment_url, bsky_agent)
                        .await?,
                }
                .into(),
            );
        } else if content_type.starts_with("video/") {
            embed =
                Some(bluesky_upload_or_embed_video(&bytes, attachment, post, bsky_agent).await?);
            break;
        }
    }
    // If there is no video then use the images.
    if embed.is_none() {
        embed = Some(bsky_sdk::api::types::Union::Refs(
            bsky_sdk::api::app::bsky::feed::post::RecordEmbedRefs::AppBskyEmbedImagesMain(
                Box::new(bsky_sdk::api::app::bsky::embed::images::MainData { images }.into()),
            ),
        ));
    }

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
        let tmp_file = NamedTempFile::new()?;
        let mut source_file = File::create(tmp_file.path()).await?;
        source_file.write_all(download_bytes).await?;
        // Try with 100% quality first, then decrease by 10% until we
        // get less than 1MB.
        let mut quality = 100.;
        loop {
            let dest_dir = tempdir()?;
            let mut compressor = Compressor::new(tmp_file.path(), dest_dir.path());
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
                let mut compressed_file = File::open(compressed).await?;
                let mut compressed_bytes = Vec::new();
                compressed_file.read_to_end(&mut compressed_bytes).await?;
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

// Before uploading a video to Bluesky, we need to check if it is less than 60
// seconds. When it is longer we embed it as external post instead.
async fn bluesky_upload_or_embed_video(
    video_bytes: &[u8],
    attachment: &NewMedia,
    post: &NewStatus,
    bsky_agent: &BskyAgent,
) -> Result<bsky_sdk::api::types::Union<RecordEmbedRefs>> {
    // Save video bytes to a temporary file and check if it is less than
    // 60 seconds.
    let tmp_file = NamedTempFile::new()?;
    let mut video_file = File::create(tmp_file.path()).await?;
    video_file.write_all(video_bytes).await?;
    let ffprobe_output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(tmp_file.path())
        .output()
        .context(format!(
            "Failed to execute ffprobe for video {}",
            attachment.attachment_url
        ))?;
    let duration: f64 = String::from_utf8_lossy(&ffprobe_output.stdout)
        .trim()
        .parse()
        .context(format!(
            "Failed to parse ffprobe output for video {}",
            attachment.attachment_url
        ))?;
    // If the video is longer then embed the original toot as link
    // embed.
    if duration > 60. {
        let response = reqwest::get(&post.original_post_url)
            .await
            .context(format!(
                "Failed extracting link preview {}",
                post.original_post_url
            ))?;
        let html_bytes = response.bytes().await?;
        let html = webpage::HTML::from_string(
            String::from_utf8_lossy(&html_bytes).to_string(),
            Some(post.original_post_url.clone()),
        )
        .context(format!(
            "Failed parsing HTML from {}",
            post.original_post_url
        ))?;
        let thumb = match html.opengraph.images.first() {
            Some(image) => {
                let thumb_bytes = reqwest::get(&image.url)
                    .await
                    .context(format!("Failed downloading thumbnail {}", image.url))?
                    .bytes()
                    .await?;
                Some(bluesky_upload_image(&thumb_bytes, &image.url, bsky_agent).await?)
            }
            None => None,
        };
        let external = bsky_sdk::api::app::bsky::embed::external::MainData {
            external: bsky_sdk::api::app::bsky::embed::external::ExternalData {
                description: html
                    .opengraph
                    .properties
                    .get("description")
                    .unwrap_or(&"".to_string())
                    .to_string(),
                thumb,
                title: html
                    .opengraph
                    .properties
                    .get("title")
                    .unwrap_or(&"".to_string())
                    .to_string(),
                uri: post.original_post_url.clone(),
            }
            .into(),
        };
        Ok(bsky_sdk::api::types::Union::Refs(
            bsky_sdk::api::app::bsky::feed::post::RecordEmbedRefs::AppBskyEmbedExternalMain(
                Box::new(external.into()),
            ),
        ))
    } else {
        let blob = bluesky_upload_video(bsky_agent, &attachment.attachment_url, video_bytes.into())
            .await?;
        let video = bsky_sdk::api::app::bsky::embed::video::MainData {
            alt: attachment.alt_text.clone(),
            aspect_ratio: None,
            captions: None,
            video: blob,
        };
        Ok(bsky_sdk::api::types::Union::Refs(
            bsky_sdk::api::app::bsky::feed::post::RecordEmbedRefs::AppBskyEmbedVideoMain(Box::new(
                video.into(),
            )),
        ))
    }
}

async fn bluesky_upload_image(
    image_bytes: &[u8],
    image_url: &str,
    bsky_agent: &BskyAgent,
) -> Result<BlobRef> {
    let attachment_bytes = resize_image_if_needed(image_bytes, image_url).await?;

    let output = bsky_agent
        .api
        .com
        .atproto
        .repo
        .upload_blob(attachment_bytes)
        .await
        .context(format!("Failed uploading image to Bluesky {}", image_url))?;
    Ok(output.data.blob)
}
