//! [`VideoProvider`] over live memory plus [`IndexedVideoDvr`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use a2a_lab_dev_kit::{
    A2aLabError, DvrFrameAtRequest, DvrMediaPackager, DvrMediaPayload, DvrMediaStore,
    FollowVideoRequest, GetVideoClipRequest, GetVideoFrameAtRequest, GetVideoFrameAtResult,
    GetVideoTimelineRequest, Image, ImageDescriptor, ImageId, ImageSourceId, JsonObject,
    ListVideoSourcesRequest, MAX_PAGE_LIMIT, MemoryVideos, PackagedMediaAsset, Page, PageRequest,
    ReadDvrMediaWindowRequest, ResolvedMediaAsset, TimeRange, UtcTimestamp, VideoClip, VideoClipId,
    VideoClipStream, VideoFollow, VideoFollowId, VideoFrame, VideoLiveStream, VideoProvider,
    VideoSource, VideoSourceId, VideoTimeline,
};
use tokio::sync::Mutex;

use crate::images::jpeg_dimensions;
use crate::videos::dvr::IndexedVideoDvr;

use a2a_lab_dev_kit::DvrTrackId;

#[derive(Clone)]
struct StoredClip {
    clip: VideoClip,
    track_id: DvrTrackId,
    expires_at: Instant,
}

/// Combined live window and disk-backed DVR catalog.
#[derive(Clone)]
pub struct VideoDvrCatalog {
    live: MemoryVideos,
    dvr: IndexedVideoDvr,
    clips: Arc<Mutex<Vec<StoredClip>>>,
    next_clip: Arc<Mutex<u64>>,
    clip_ttl: Duration,
}

impl VideoDvrCatalog {
    pub fn open(
        live: MemoryVideos,
        dvr: IndexedVideoDvr,
        clip_ttl: Duration,
    ) -> Result<Self, A2aLabError> {
        Ok(Self {
            live,
            dvr,
            clips: Arc::new(Mutex::new(Vec::new())),
            next_clip: Arc::new(Mutex::new(0)),
            clip_ttl,
        })
    }

    #[must_use]
    pub fn live(&self) -> &MemoryVideos {
        &self.live
    }

    #[must_use]
    pub fn dvr(&self) -> &IndexedVideoDvr {
        &self.dvr
    }

    pub async fn cleanup_disk(&self, now: UtcTimestamp) -> Result<(), A2aLabError> {
        self.dvr.cleanup_at(now).await?;
        self.purge_expired_clips().await;
        Ok(())
    }

    /// Periodic disk cleanup until the returned guard is dropped.
    #[must_use]
    pub fn maintain_disk(&self) -> VideoDvrMaintenance {
        let catalog = self.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(super::VIDEO_DVR_CLEANUP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Ok(now) = crate::time::now()
                    && let Err(error) = catalog.cleanup_disk(now).await
                {
                    eprintln!("video DVR: {error}");
                }
            }
        });
        VideoDvrMaintenance { task }
    }

    async fn purge_expired_clips(&self) {
        let now = Instant::now();
        self.clips.lock().await.retain(|clip| clip.expires_at > now);
    }

    async fn effective_range(
        &self,
        source_id: &VideoSourceId,
        track_id: &DvrTrackId,
        range: TimeRange,
    ) -> Result<TimeRange, A2aLabError> {
        let mut cursor = None;
        let mut first = None;
        let mut last_end = None;
        loop {
            let page = self
                .dvr
                .read_media_window(ReadDvrMediaWindowRequest::new(
                    source_id.clone(),
                    track_id.clone(),
                    range,
                    PageRequest::new(cursor, MAX_PAGE_LIMIT)?,
                )?)
                .await?;
            for segment in page.items() {
                if first.is_none() {
                    first = Some(segment.range().start());
                }
                last_end = Some(segment.range().end());
            }
            cursor = page.next_cursor().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        let Some(start) = first else {
            return Err(A2aLabError::no_recording(
                "retained range contains no segments",
            ));
        };
        TimeRange::new(start, last_end.unwrap_or(start))
    }
}

pub struct VideoDvrMaintenance {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for VideoDvrMaintenance {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl VideoProvider for VideoDvrCatalog {
    async fn list_video_sources(
        &self,
        request: ListVideoSourcesRequest,
    ) -> Result<Page<VideoSource>, A2aLabError> {
        self.live.list_video_sources(request).await
    }

    async fn get_video_frame_at(
        &self,
        request: GetVideoFrameAtRequest,
    ) -> Result<GetVideoFrameAtResult, A2aLabError> {
        if let Ok(found) = self.live.get_video_frame_at(request.clone()).await {
            return Ok(found);
        }
        let track_id = MemoryVideos::mjpeg_track_id(request.source_id())?;
        let frame = self
            .dvr
            .frame_at(DvrFrameAtRequest::new(
                request.source_id().clone(),
                track_id,
                request.at(),
                request.max_delta_us(),
            ))
            .await?;
        let jpeg = match frame.payload() {
            DvrMediaPayload::MjpegFrame(bytes) => bytes.clone(),
            DvrMediaPayload::EncodedSample { .. } => {
                return Err(A2aLabError::invalid(
                    "codec",
                    "MJPEG frame-at requires an MJPEG track",
                ));
            }
        };
        let (width, height) = jpeg_dimensions(&jpeg)?;
        let descriptor = ImageDescriptor::new(
            ImageId::new(format!(
                "{}-{}",
                request.source_id(),
                frame.actual_at().as_micros()
            ))?,
            ImageSourceId::new(request.source_id().as_str())?,
            frame.actual_at(),
            "image/jpeg",
            width,
            height,
            None,
            JsonObject::empty(),
        )?;
        Ok(GetVideoFrameAtResult::new(
            request.at(),
            Image::new(descriptor, jpeg)?,
        ))
    }

    async fn get_video_clip(&self, request: GetVideoClipRequest) -> Result<VideoClip, A2aLabError> {
        request.check()?;
        let source_id = request.source_id().clone();
        let track_id = MemoryVideos::mjpeg_track_id(&source_id)?;
        let Ok(effective_range) = self
            .effective_range(&source_id, &track_id, request.range())
            .await
        else {
            return self.live.get_video_clip(request).await;
        };
        let mut next = self.next_clip.lock().await;
        *next += 1;
        let id = VideoClipId::new(format!("clip-{next}"))?;
        let clip = VideoClip::new_mjpeg(id, source_id, request.range(), effective_range)?;
        let expires_at = Instant::now() + self.clip_ttl;
        self.clips.lock().await.push(StoredClip {
            clip: clip.clone(),
            track_id,
            expires_at,
        });
        Ok(clip)
    }

    async fn open_video_clip(&self, id: &VideoClipId) -> Result<VideoClipStream, A2aLabError> {
        let stored = self
            .clips
            .lock()
            .await
            .iter()
            .find(|clip| clip.clip.id() == id)
            .cloned()
            .ok_or_else(|| A2aLabError::not_found("video clip", id.to_string()))?;
        if stored.expires_at <= Instant::now() {
            return Err(A2aLabError::clip_expired(format!(
                "video clip `{id}` has expired"
            )));
        }
        self.purge_expired_clips().await;
        let range = stored.clip.effective_range();
        let mut cursor = None;
        let mut frames = Vec::new();
        loop {
            let page = self
                .dvr
                .read_media_window(ReadDvrMediaWindowRequest::new(
                    stored.clip.source_id().clone(),
                    stored.track_id.clone(),
                    range,
                    PageRequest::new(cursor, MAX_PAGE_LIMIT)?,
                )?)
                .await?;
            for segment in page.items() {
                if let DvrMediaPayload::MjpegFrame(jpeg) = segment.payload() {
                    frames.push(VideoFrame::new(segment.range().start(), jpeg.clone())?);
                }
            }
            cursor = page.next_cursor().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        if frames.is_empty() {
            return self.live.open_video_clip(id).await;
        }
        Ok(Box::pin(futures_util::stream::iter(
            frames.into_iter().map(Ok),
        )))
    }

    async fn export_finite_video_clip(
        &self,
        id: &VideoClipId,
    ) -> Result<PackagedMediaAsset, A2aLabError> {
        let stored = self
            .clips
            .lock()
            .await
            .iter()
            .find(|clip| clip.clip.id() == id)
            .cloned()
            .ok_or_else(|| A2aLabError::not_found("video clip", id.to_string()))?;
        if stored.expires_at <= Instant::now() {
            return Err(A2aLabError::clip_expired(format!(
                "video clip `{id}` has expired"
            )));
        }
        self.purge_expired_clips().await;
        let packager = DvrMediaPackager::default();
        packager
            .export_finite_mjpeg(
                &self.dvr,
                stored.clip.source_id(),
                &stored.track_id,
                stored.clip.effective_range(),
            )
            .await
    }

    async fn get_video_timeline(
        &self,
        request: GetVideoTimelineRequest,
    ) -> Result<VideoTimeline, A2aLabError> {
        let live = self
            .live
            .get_video_timeline(GetVideoTimelineRequest::new(request.source_id().clone()))
            .await;
        let dvr = self.dvr.media_coverage(request.source_id()).await;
        match (live, dvr) {
            (Ok(live_tl), Ok(coverage)) => {
                let newest = live_tl.newest_at().or_else(|| {
                    coverage
                        .intervals()
                        .last()
                        .map(|interval| interval.end().saturating_add_micros(-1))
                });
                VideoTimeline::new(
                    request.source_id().clone(),
                    coverage.intervals().to_vec(),
                    newest,
                    live_tl.live_available(),
                )
            }
            (Ok(live_tl), Err(_)) => Ok(live_tl),
            (Err(_), Ok(coverage)) => VideoTimeline::new(
                request.source_id().clone(),
                coverage.intervals().to_vec(),
                coverage
                    .intervals()
                    .last()
                    .map(|interval| interval.end().saturating_add_micros(-1)),
                false,
            ),
            (Err(error), Err(_)) => Err(error),
        }
    }

    async fn follow_video(&self, request: FollowVideoRequest) -> Result<VideoFollow, A2aLabError> {
        self.live.follow_video(request).await
    }

    async fn open_video_follow(&self, id: &VideoFollowId) -> Result<VideoLiveStream, A2aLabError> {
        self.live.open_video_follow(id).await
    }

    async fn rebase_video_follow(
        &self,
        id: &VideoFollowId,
        anchor: UtcTimestamp,
    ) -> Result<VideoFollow, A2aLabError> {
        self.live.rebase_video_follow(id, anchor).await
    }

    async fn resolve_live_playback_asset(
        &self,
        id: &VideoFollowId,
        relative_path: Option<&str>,
    ) -> Result<ResolvedMediaAsset, A2aLabError> {
        self.live
            .resolve_live_playback_asset(id, relative_path)
            .await
    }
}

#[cfg(test)]
impl VideoDvrCatalog {
    pub async fn expire_clips(&self) {
        let now = Instant::now();
        for clip in self.clips.lock().await.iter_mut() {
            clip.expires_at = now;
        }
    }
}
