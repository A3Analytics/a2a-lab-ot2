//! Epoch-aware H.264 packaging over [`IndexedVideoDvr`].

use a2a_lab_dev_kit::{
    A2aLabError, DvrMediaStore, EpochHlsSession, EpochPlaylistKind, H264LiveStore,
    HLS_PLAYLIST_RELATIVE_PATH, HLS_TRICK_8X_PLAYLIST_RELATIVE_PATH, MAX_PAGE_LIMIT,
    PackagedMediaAsset, PageRequest, ReadDvrMediaWindowRequest, TRICK_8X_UNAVAILABLE_NO_REDUCTION,
    TimeRange, UtcTimestamp, VideoCodec, VideoPlayback8xMetadata, VideoSourceId,
    live_h264_edge_anchor, load_live_hls_asset, package_h264_epochs, package_h264_live_at,
};

use super::IndexedVideoDvr;

pub(crate) async fn effective_h264_range(
    dvr: &IndexedVideoDvr,
    source_id: &VideoSourceId,
    range: TimeRange,
) -> Result<TimeRange, A2aLabError> {
    range.check()?;
    let tracks = dvr.list_tracks(source_id).await?;
    let epochs: Vec<_> = tracks
        .iter()
        .filter(|track| track.codec() == VideoCodec::H264)
        .filter(|track| track.id().as_str() != "h264")
        .collect();
    if epochs.is_empty() {
        return Err(A2aLabError::no_recording(
            "retained range contains no H.264 epochs",
        ));
    }
    let mut first = None;
    let mut last_end = None;
    for track in epochs {
        if track.bounds().end() <= range.start() || track.bounds().start() >= range.end() {
            continue;
        }
        let window = TimeRange::new(
            range.start().max(track.bounds().start()),
            range.end().min(track.bounds().end()),
        )?;
        let mut cursor = None;
        loop {
            let page = dvr
                .read_media_window(ReadDvrMediaWindowRequest::new(
                    source_id.clone(),
                    track.id().clone(),
                    window,
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
    }
    let Some(start) = first else {
        return Err(A2aLabError::no_recording(
            "retained range contains no segments",
        ));
    };
    TimeRange::new(start, last_end.unwrap_or(start))
}

pub(crate) async fn package_epochs_range(
    dvr: &IndexedVideoDvr,
    source_id: &VideoSourceId,
    range: TimeRange,
    kind: EpochPlaylistKind,
) -> Result<EpochHlsSession, A2aLabError> {
    package_h264_epochs(dvr, source_id, range, kind).await
}

pub(crate) fn package_live_store_at(
    live: &H264LiveStore,
    anchor: UtcTimestamp,
    playback_window_us: u64,
) -> Result<EpochHlsSession, A2aLabError> {
    let snapshot = live.snapshot()?;
    package_h264_live_at(&snapshot, anchor, playback_window_us)
}

pub(crate) fn live_store_edge_anchor(live: &H264LiveStore) -> Result<UtcTimestamp, A2aLabError> {
    let snapshot = live.snapshot()?;
    live_h264_edge_anchor(&snapshot)
}

pub(crate) fn load_live_store_asset(
    live: &H264LiveStore,
    session: &EpochHlsSession,
    relative_path: &str,
) -> Result<PackagedMediaAsset, A2aLabError> {
    let snapshot = live.snapshot()?;
    load_live_hls_asset(&snapshot, session, relative_path)
}

pub(crate) fn epoch_session_has(session: &EpochHlsSession, path: &str) -> bool {
    if path == HLS_PLAYLIST_RELATIVE_PATH {
        return true;
    }
    if path == HLS_TRICK_8X_PLAYLIST_RELATIVE_PATH {
        return session.trick_playlist().is_some();
    }
    if session
        .inits()
        .iter()
        .any(|init| init.relative_path() == path)
    {
        return true;
    }
    session
        .segments()
        .iter()
        .any(|segment| segment.relative_path() == path)
}

pub(crate) fn playback_8x_from_epoch(session: &EpochHlsSession) -> VideoPlayback8xMetadata {
    if session.trick_playlist().is_some() {
        VideoPlayback8xMetadata::available(
            HLS_TRICK_8X_PLAYLIST_RELATIVE_PATH,
            HLS_PLAYLIST_RELATIVE_PATH,
        )
    } else {
        VideoPlayback8xMetadata::unavailable(
            HLS_PLAYLIST_RELATIVE_PATH,
            TRICK_8X_UNAVAILABLE_NO_REDUCTION,
        )
    }
}
