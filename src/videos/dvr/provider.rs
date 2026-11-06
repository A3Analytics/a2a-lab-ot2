//! [`VideoProvider`] over live memory plus [`IndexedVideoDvr`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use a2a_lab_dev_kit::{
    A2aLabError, DvrFrameAtRequest, DvrMediaPackager, DvrMediaPayload, DvrMediaStore,
    FollowVideoRequest, GetVideoClipRequest, GetVideoFrameAtRequest, GetVideoFrameAtResult,
    GetVideoTimelineRequest, HLS_PLAYLIST_RELATIVE_PATH, Image, ImageDescriptor, ImageId,
    ImageSourceId, JsonObject, ListVideoSourcesRequest, MAX_PAGE_LIMIT, MemoryVideos,
    PackagedHlsSession, PackagedMediaAsset, Page, PageRequest, ReadDvrMediaWindowRequest,
    ResolvedMediaAsset, TimeRange, UtcTimestamp, VideoClip, VideoClipId, VideoClipStream,
    VideoCodec, VideoFollow, VideoFollowId, VideoFrame, VideoLiveStream, VideoProvider,
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
    hls: Option<PackagedHlsSession>,
}

#[derive(Clone)]
struct StoredFollow {
    follow: VideoFollow,
    source_id: VideoSourceId,
    anchor: Option<UtcTimestamp>,
    expires_at: Instant,
}

/// Combined live window and disk-backed DVR catalog.
#[derive(Clone)]
pub struct VideoDvrCatalog {
    live: MemoryVideos,
    dvr: IndexedVideoDvr,
    clips: Arc<Mutex<Vec<StoredClip>>>,
    next_clip: Arc<Mutex<u64>>,
    follows: Arc<Mutex<Vec<StoredFollow>>>,
    next_follow: Arc<Mutex<u64>>,
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
            follows: Arc::new(Mutex::new(Vec::new())),
            next_follow: Arc::new(Mutex::new(0)),
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

    async fn h264_ready(&self, source_id: &VideoSourceId) -> bool {
        let Ok(true) = self.live.live_available(source_id).await else {
            return false;
        };
        let Ok(tracks) = self.dvr.list_tracks(source_id).await else {
            return false;
        };
        tracks
            .iter()
            .any(|track| track.id().as_str() == "h264" && track.codec() == VideoCodec::H264)
    }

    async fn follow_encoded(
        &self,
        request: FollowVideoRequest,
    ) -> Result<VideoFollow, A2aLabError> {
        let mut next = self.next_follow.lock().await;
        *next += 1;
        let id = VideoFollowId::new(format!("h264-follow-{next}"))?;
        drop(next);
        let mut follow =
            VideoFollow::new_encoded(id, request.source_id().clone(), VideoCodec::H264)?;
        let packager = DvrMediaPackager::default();
        let anchor = match request.anchor_at() {
            Some(anchor) => Ok(anchor),
            None => DvrMediaPackager::live_edge_anchor(&self.dvr, request.source_id()).await,
        };
        if let Ok(anchor) = anchor
            && let Ok(session) = packager
                .package_live_hls_at(
                    &self.dvr,
                    follow.path(),
                    request.source_id(),
                    &DvrTrackId::new("h264")?,
                    anchor,
                )
                .await
        {
            follow = follow.with_playback_8x(session.playback_8x_metadata());
        }
        self.follows.lock().await.push(StoredFollow {
            follow: follow.clone(),
            source_id: request.source_id().clone(),
            anchor: request.anchor_at(),
            expires_at: Instant::now() + self.clip_ttl,
        });
        Ok(follow)
    }

    async fn encoded_follow(&self, id: &VideoFollowId) -> Option<StoredFollow> {
        self.follows
            .lock()
            .await
            .iter()
            .find(|follow| follow.follow.id() == id)
            .cloned()
    }

    async fn load_encoded_asset(
        &self,
        id: &VideoFollowId,
        relative_path: Option<&str>,
    ) -> Result<ResolvedMediaAsset, A2aLabError> {
        let stored = self
            .encoded_follow(id)
            .await
            .ok_or_else(|| A2aLabError::not_found("video follow", id.to_string()))?;
        if stored.expires_at <= Instant::now() {
            return Err(A2aLabError::clip_expired(format!(
                "live session `{id}` has expired"
            )));
        }
        let track_id = DvrTrackId::new("h264")?;
        let packager = DvrMediaPackager::default();
        let anchor = match stored.anchor {
            Some(anchor) => anchor,
            None => DvrMediaPackager::live_edge_anchor(&self.dvr, &stored.source_id).await?,
        };
        let session = packager
            .package_live_hls_at(
                &self.dvr,
                stored.follow.path(),
                &stored.source_id,
                &track_id,
                anchor,
            )
            .await?;
        let path = relative_path.unwrap_or(HLS_PLAYLIST_RELATIVE_PATH);
        let asset = packager.load_hls_asset(&self.dvr, &session, path).await?;
        Ok(ResolvedMediaAsset::Bytes(asset))
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
        let h264 = DvrTrackId::new("h264")?;
        let (track_id, encoded) = if self
            .effective_range(&source_id, &h264, request.range())
            .await
            .is_ok()
        {
            (h264, true)
        } else {
            (MemoryVideos::mjpeg_track_id(&source_id)?, false)
        };
        let Ok(effective_range) = self
            .effective_range(&source_id, &track_id, request.range())
            .await
        else {
            return self.live.get_video_clip(request).await;
        };
        let mut next = self.next_clip.lock().await;
        *next += 1;
        let id = VideoClipId::new(format!("clip-{next}"))?;
        let mut clip = if encoded {
            VideoClip::new(
                id,
                source_id.clone(),
                request.range(),
                effective_range,
                VideoCodec::H264,
                "application/vnd.apple.mpegurl",
            )?
        } else {
            VideoClip::new_mjpeg(id, source_id, request.range(), effective_range)?
        };
        let hls = if encoded {
            let session = DvrMediaPackager::default()
                .package_hls_range(
                    &self.dvr,
                    clip.path(),
                    clip.source_id(),
                    &track_id,
                    effective_range,
                )
                .await?;
            clip = clip.with_playback_8x(session.playback_8x_metadata());
            Some(session)
        } else {
            None
        };
        let expires_at = Instant::now() + self.clip_ttl;
        self.clips.lock().await.push(StoredClip {
            clip: clip.clone(),
            track_id,
            expires_at,
            hls,
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
        if stored.clip.codec() == VideoCodec::H264 {
            return packager
                .export_finite_mp4(
                    &self.dvr,
                    stored.clip.source_id(),
                    &stored.track_id,
                    stored.clip.effective_range(),
                )
                .await;
        }
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
                let intervals = coverage.intervals().to_vec();
                let newest = intervals
                    .last()
                    .map(|interval| newest_inside(*interval, live_tl.newest_at()));
                VideoTimeline::new(
                    request.source_id().clone(),
                    intervals,
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
        if self.h264_ready(request.source_id()).await {
            return self.follow_encoded(request).await;
        }
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
        let mut follows = self.follows.lock().await;
        if let Some(stored) = follows.iter_mut().find(|follow| follow.follow.id() == id) {
            if stored.expires_at <= Instant::now() {
                return Err(A2aLabError::clip_expired(format!(
                    "live session `{id}` has expired"
                )));
            }
            stored.anchor = Some(anchor);
            return Ok(stored.follow.clone());
        }
        drop(follows);
        self.live.rebase_video_follow(id, anchor).await
    }

    async fn resolve_live_playback_asset(
        &self,
        id: &VideoFollowId,
        relative_path: Option<&str>,
    ) -> Result<ResolvedMediaAsset, A2aLabError> {
        if self.encoded_follow(id).await.is_some() {
            return self.load_encoded_asset(id, relative_path).await;
        }
        self.live
            .resolve_live_playback_asset(id, relative_path)
            .await
    }

    async fn resolve_clip_playback_asset(
        &self,
        id: &VideoClipId,
        relative_path: Option<&str>,
    ) -> Result<ResolvedMediaAsset, A2aLabError> {
        let path = relative_path.unwrap_or(HLS_PLAYLIST_RELATIVE_PATH);
        let packager = DvrMediaPackager::default();
        let cached = {
            let mut clips = self.clips.lock().await;
            let Some(stored) = clips.iter_mut().find(|clip| clip.clip.id() == id) else {
                return Err(A2aLabError::not_found("video clip", id.to_string()));
            };
            if stored.clip.codec() != VideoCodec::H264 {
                return Err(A2aLabError::not_found("video clip", id.to_string()));
            }
            if stored.expires_at <= Instant::now() {
                return Err(A2aLabError::clip_expired(format!(
                    "video clip `{id}` has expired"
                )));
            }
            stored.expires_at = Instant::now() + self.clip_ttl;
            stored
                .hls
                .clone()
                .filter(|session| clip_session_has(session, path))
        };
        if let Some(session) = cached.as_ref() {
            let asset = packager.load_hls_asset(&self.dvr, session, path).await?;
            return Ok(ResolvedMediaAsset::Bytes(asset));
        }
        let (root, source_id, track_id, range) = {
            let clips = self.clips.lock().await;
            let Some(stored) = clips.iter().find(|clip| clip.clip.id() == id) else {
                return Err(A2aLabError::not_found("video clip", id.to_string()));
            };
            (
                stored.clip.path().to_owned(),
                stored.clip.source_id().clone(),
                stored.track_id.clone(),
                stored.clip.effective_range(),
            )
        };
        let session = packager
            .package_hls_range(&self.dvr, &root, &source_id, &track_id, range)
            .await?;
        let asset = packager.load_hls_asset(&self.dvr, &session, path).await?;
        if let Some(stored) = self
            .clips
            .lock()
            .await
            .iter_mut()
            .find(|clip| clip.clip.id() == id)
        {
            stored.hls = Some(session);
            stored.expires_at = Instant::now() + self.clip_ttl;
        }
        Ok(ResolvedMediaAsset::Bytes(asset))
    }
}

/// Live memory can sit one frame past the newest indexed interval while the
/// disk writer catches up. `newest_at` has to stay inside that interval.
fn newest_inside(interval: TimeRange, live_newest: Option<UtcTimestamp>) -> UtcTimestamp {
    live_newest
        .filter(|at| interval.contains(*at))
        .unwrap_or_else(|| interval.end().saturating_add_micros(-1))
}

fn clip_session_has(session: &PackagedHlsSession, path: &str) -> bool {
    if path == session.init_relative_path() || path == session.playlist_relative_path() {
        return true;
    }
    if session
        .segments()
        .iter()
        .any(|segment| segment.relative_path() == path)
    {
        return true;
    }
    session.trick_8x().is_some_and(|trick| {
        path == trick.playlist_relative_path()
            || trick
                .segments()
                .iter()
                .any(|segment| segment.relative_path() == path)
    })
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

#[cfg(test)]
mod tests {
    use super::newest_inside;
    use a2a_lab_dev_kit::{TimeRange, UtcTimestamp};

    fn at(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).unwrap()
    }

    fn range() -> TimeRange {
        TimeRange::new(at("2024-06-01T00:00:00Z"), at("2024-06-01T00:00:01Z")).unwrap()
    }

    #[test]
    fn newest_stays_inside_when_live_runs_ahead_of_disk() {
        let interval = range();
        let ahead = newest_inside(interval, Some(at("2024-06-01T00:00:01Z")));
        assert!(interval.contains(ahead));
        let inside = newest_inside(interval, Some(at("2024-06-01T00:00:00.500000Z")));
        assert_eq!(inside, at("2024-06-01T00:00:00.500000Z"));
    }
}
