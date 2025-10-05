use crate::BskyAgent;
use anyhow::{Result, bail};
use atrium_xrpc_client::reqwest::ReqwestClient;
use bsky_sdk::api::{
    client::AtpServiceClient,
    types::{BlobRef, string::Did},
    xrpc::{
        HttpClient, XrpcClient,
        http::{Request, Response, uri::Builder},
        types::AuthorizationToken,
    },
};
use serde::Serialize;
use std::result::Result::Ok;
use std::time::Duration;
use tokio::time;
use url::Url;

const VIDEO_SERVICE: &str = "https://video.bsky.app";
const UPLOAD_VIDEO_PATH: &str = "/xrpc/app.bsky.video.uploadVideo";

#[derive(Serialize)]
struct UploadParams {
    did: Did,
    name: String,
}

struct VideoClient {
    token: String,
    params: Option<UploadParams>,
    inner: ReqwestClient,
}

impl VideoClient {
    fn new(token: String, params: Option<UploadParams>) -> Self {
        Self {
            token,
            params,
            inner: ReqwestClient::new(
                // Actually, `base_uri` returns `VIDEO_SERVICE`, so there is no need to specify this.
                "https://dummy.example.com",
            ),
        }
    }
}

impl HttpClient for VideoClient {
    async fn send_http(
        &self,
        mut request: Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let is_upload_video = request.uri().path() == UPLOAD_VIDEO_PATH;
        // Hack: Append query parameters
        if is_upload_video {
            if let Some(params) = &self.params {
                *request.uri_mut() = Builder::from(request.uri().clone())
                    .path_and_query(format!(
                        "{UPLOAD_VIDEO_PATH}?{}",
                        serde_html_form::to_string(params)?
                    ))
                    .build()?;
            }
        }
        let mut response = self.inner.send_http(request).await;
        // Hack: Formatting an incorrect response body
        if is_upload_video {
            if let Ok(res) = response.as_mut() {
                *res.body_mut() = [
                    b"{\"jobStatus\":".to_vec(),
                    res.body().to_vec(),
                    b"}".to_vec(),
                ]
                .concat();
            }
        }
        response
    }
}

impl XrpcClient for VideoClient {
    fn base_uri(&self) -> String {
        VIDEO_SERVICE.to_string()
    }
    async fn authorization_token(&self, _: bool) -> Option<AuthorizationToken> {
        Some(AuthorizationToken::Bearer(self.token.clone()))
    }
}

// Upload a video to Bluesky and wait for it to be processed.
// Code copied from
// https://github.com/sugyan/atrium/blob/main/examples/video/src/main.rs
pub async fn bluesky_upload_video(
    bsky_agent: &BskyAgent,
    url: &str,
    video_bytes: Vec<u8>,
) -> Result<BlobRef> {
    println!("Uploading video {url} to Bluesky...");
    let session = bsky_agent.get_session().await.unwrap();
    let output = {
        let service_auth = bsky_agent
            .api
            .com
            .atproto
            .server
            .get_service_auth(
                bsky_sdk::api::com::atproto::server::get_service_auth::ParametersData {
                    aud: format!(
                        "did:web:{}",
                        bsky_agent
                            .get_endpoint()
                            .await
                            .strip_prefix("https://")
                            .unwrap()
                    )
                    .parse()
                    .expect("invalid DID"),
                    exp: None,
                    lxm: bsky_sdk::api::com::atproto::repo::upload_blob::NSID
                        .parse()
                        .ok(),
                }
                .into(),
            )
            .await?;

        let video_url = Url::parse(url)?;
        let filename = video_url
            .path_segments()
            .unwrap()
            .last()
            .filter(|&s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or("video.mp4".to_string());
        let client = AtpServiceClient::new(VideoClient::new(
            service_auth.data.token,
            Some(UploadParams {
                did: session.did.clone(),
                name: filename,
            }),
        ));
        client
            .service
            .app
            .bsky
            .video
            .upload_video(video_bytes)
            .await?
    };

    // Wait for the video to be uploaded
    let client = AtpServiceClient::new(ReqwestClient::new(VIDEO_SERVICE));
    let mut status = output.data.job_status.data;
    loop {
        status = client
            .service
            .app
            .bsky
            .video
            .get_job_status(
                bsky_sdk::api::app::bsky::video::get_job_status::ParametersData {
                    job_id: status.job_id.clone(),
                }
                .into(),
            )
            .await?
            .data
            .job_status
            .data;
        let state = &status.state;
        println!("Video status: {state}");
        if status.blob.is_some()
            || status.state == "JOB_STATE_COMPLETED"
            || status.state == "JOB_STATE_FAILED"
        {
            break;
        }
        time::sleep(Duration::from_secs(1)).await;
    }
    let Some(video) = status.blob else {
        bail!("Failed to get video blob: {status:?}");
    };
    println!("Video {url} uploaded to Bluesky");
    Ok(video)
}
