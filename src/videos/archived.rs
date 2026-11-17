//! Read-only video from a configured HTTP archive.

use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, DEFAULT_MAX_IMAGE_BYTES, GetVideoClipRequest, GetVideoFrameAtRequest,
    GetVideoFrameAtResult, GetVideoTimelineRequest, Image, ImageDescriptor, ImageId, ImageSourceId,
    JsonObject, ListVideoSourcesRequest, PackagedMediaAsset, Page, ResolvedMediaAsset,
    UtcTimestamp, VideoClip, VideoClipId, VideoClipStream, VideoProvider, VideoSource,
    VideoTimeline,
};
use reqwest::{Client, Response, Url, header};
use serde::de::DeserializeOwned;

const MAX_ASSET_BYTES: usize = 24 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 1024 * 1024;

pub struct ArchivedVideos {
    base: Url,
    http: Client,
}

impl ArchivedVideos {
    pub fn new(address: &str) -> Result<Self, A2aLabError> {
        let base = Url::parse(address.trim())
            .map_err(|_| A2aLabError::invalid("video archive URL", "expected an HTTP URL"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(A2aLabError::invalid(
                "video archive URL",
                "expected an HTTP URL without credentials, a query, or a fragment",
            ));
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(12))
            .build()
            .map_err(|_| {
                A2aLabError::unavailable("could not initialize the video archive client")
            })?;
        Ok(Self { base, http })
    }

    async fn get(&self, path: &[&str], query: &[(&str, String)]) -> Result<Response, A2aLabError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| A2aLabError::invalid("video archive URL", "cannot append path"))?
            .pop_if_empty()
            .extend(path);
        let response = self
            .http
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|_| A2aLabError::unavailable("video archive request failed"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(A2aLabError::not_found("recorded video", path.join("/")));
        }
        response.error_for_status().map_err(|error| {
            A2aLabError::unavailable(format!(
                "video archive request failed: {}",
                error.status().unwrap_or_default()
            ))
        })
    }

    async fn json<T: DeserializeOwned>(
        &self,
        path: &[&str],
        query: &[(&str, String)],
    ) -> Result<T, A2aLabError> {
        let data = read_body(self.get(path, query).await?, MAX_METADATA_BYTES).await?;
        serde_json::from_slice(&data)
            .map_err(|_| A2aLabError::unavailable("video archive returned invalid metadata"))
    }
}

impl VideoProvider for ArchivedVideos {
    async fn list_video_sources(
        &self,
        request: ListVideoSourcesRequest,
    ) -> Result<Page<VideoSource>, A2aLabError> {
        let sources: Vec<VideoSource> = self.json(&["sources"], &[]).await?;
        let start: usize = request
            .page()
            .cursor()
            .unwrap_or("0")
            .parse()
            .map_err(|_| A2aLabError::invalid("cursor", "invalid page cursor"))?;
        if start > sources.len() {
            return Err(A2aLabError::invalid("cursor", "is past the end"));
        }
        let end = start
            .saturating_add(request.page().limit() as usize)
            .min(sources.len());
        let next = (end < sources.len()).then(|| end.to_string());
        Ok(Page::new(
            sources.into_iter().skip(start).take(end - start).collect(),
            next,
        ))
    }

    async fn get_video_timeline(
        &self,
        request: GetVideoTimelineRequest,
    ) -> Result<VideoTimeline, A2aLabError> {
        self.json(&["timeline", request.source_id().as_str()], &[])
            .await
    }

    async fn get_video_frame_at(
        &self,
        request: GetVideoFrameAtRequest,
    ) -> Result<GetVideoFrameAtResult, A2aLabError> {
        let source = request.source_id().as_str();
        let at = request.at();
        let response = self
            .get(&["frame", source], &[("at", at.to_rfc3339())])
            .await?;
        let captured_at = UtcTimestamp::parse(&read_header::<String>(&response, "x-captured-at")?)?;
        if let Some(limit) = request.max_delta_us()
            && captured_at.as_micros().abs_diff(at.as_micros()) > limit
        {
            return Err(A2aLabError::not_found("recorded frame", at.to_rfc3339()));
        }
        let width = read_header(&response, "x-frame-width")?;
        let height = read_header(&response, "x-frame-height")?;
        let descriptor = ImageDescriptor::new(
            ImageId::new(format!("{source}-{}", captured_at.as_micros()))?,
            ImageSourceId::new(source)?,
            captured_at,
            "image/jpeg",
            width,
            height,
            Some("Recorded replay".to_owned()),
            JsonObject::empty(),
        )?;
        let jpeg = read_body(
            response,
            usize::try_from(DEFAULT_MAX_IMAGE_BYTES).unwrap_or(usize::MAX),
        )
        .await?;
        Ok(GetVideoFrameAtResult::new(
            at,
            Image::new(descriptor, jpeg)?,
        ))
    }

    async fn get_video_clip(&self, request: GetVideoClipRequest) -> Result<VideoClip, A2aLabError> {
        let range = request.range();
        self.json(
            &["clip", request.source_id().as_str()],
            &[
                ("start", range.start().to_rfc3339()),
                ("end", range.end().to_rfc3339()),
            ],
        )
        .await
    }

    fn open_video_clip(
        &self,
        id: &VideoClipId,
    ) -> impl Future<Output = Result<VideoClipStream, A2aLabError>> {
        std::future::ready(Err(A2aLabError::not_found(
            "JPEG clip stream",
            id.to_string(),
        )))
    }

    async fn resolve_clip_playback_asset(
        &self,
        id: &VideoClipId,
        relative_path: Option<&str>,
    ) -> Result<ResolvedMediaAsset, A2aLabError> {
        let filename = relative_path.unwrap_or("index.m3u8");
        if filename.contains("..")
            || !filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return Err(A2aLabError::invalid(
                "asset",
                "expected a single media filename",
            ));
        }
        let response = self.get(&["media", id.as_str(), filename], &[]).await?;
        let content_type: String = read_header(&response, header::CONTENT_TYPE.as_str())?;
        let data = read_body(response, MAX_ASSET_BYTES).await?;
        Ok(ResolvedMediaAsset::Bytes(PackagedMediaAsset::new(
            content_type,
            data,
        )?))
    }
}

fn read_header<T: std::str::FromStr>(
    response: &Response,
    name: &'static str,
) -> Result<T, A2aLabError> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| A2aLabError::unavailable(format!("video archive returned invalid {name}")))
}

async fn read_body(mut response: Response, limit: usize) -> Result<Vec<u8>, A2aLabError> {
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| A2aLabError::unavailable("could not read video archive response"))?
    {
        if chunk.len() > limit.saturating_sub(data.len()) {
            return Err(A2aLabError::unavailable(
                "video archive response is too large",
            ));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}
