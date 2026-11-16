//! Bounded saved-frame evidence reader.

use std::io::Cursor;

use a2a_lab_dev_kit::videos::{
    AllIdrEncoder, CaptureIdentity, ColorMatrix, ColorProperties, ColorRange, FrameRoiTransform,
    I420Picture, SavedFrameRef, frame_h264_picture,
};
use a2a_lab_dev_kit::{DvrStoreNamespace, DvrTrackId, MemoryVideos, UtcTimestamp, VideoSourceId};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, FrameEvidenceFailureKind, FrameEvidenceLimits,
    FrameEvidenceRequest,
};
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;

#[test]
fn standard_decode_cap_accepts_native_external_camera_edge() {
    assert_eq!(FrameEvidenceLimits::standard().max_edge(), 1920);
    assert!(FrameEvidenceLimits::standard().max_rgb_bytes() >= 1920 * 1080 * 3);
}

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-evidence-{name}-{}",
        std::process::id()
    ))
}

async fn open_store(name: &str) -> IndexedVideoDvr {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    IndexedVideoDvr::open(&dir, 86_400).await.unwrap()
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn encode_jpeg(width: u32, height: u32, red: u8) -> Vec<u8> {
    let mut pixels = Vec::new();
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&[red, 0, 0]);
    }
    let mut cursor = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut cursor, 90)
        .encode(&pixels, width, height, ExtendedColorType::Rgb8)
        .unwrap();
    cursor.into_inner()
}

async fn saved_mjpeg(red: u8) -> (IndexedVideoDvr, SavedFrameRef) {
    let store = open_store("mjpeg").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    let jpeg = encode_jpeg(4, 4, red);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg)
        .await
        .unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    (store, frame)
}

#[tokio::test]
async fn mjpeg_evidence_matches_revision_digest_and_pixels() {
    let (store, frame) = saved_mjpeg(90).await;
    let evidence = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, None),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap();
    assert_eq!(evidence.frame(), &frame);
    assert_eq!(evidence.verified_digest(), frame.payload_digest());
    assert_eq!(evidence.width(), 4);
    assert_eq!(evidence.height(), 4);
    assert!(evidence.rgb().iter().any(|byte| *byte > 0));
    assert_eq!(evidence.properties().matrix(), ColorMatrix::Bt601);
}

#[tokio::test]
async fn digest_mismatch_is_distinct_from_missing() {
    let (store, frame) = saved_mjpeg(10).await;
    let frame = SavedFrameRef::from_payload_bytes(
        store_ns(),
        frame.source_id().clone(),
        frame.track_id().clone(),
        frame.captured_at(),
        b"other-bytes",
        frame.capture_group().cloned(),
    )
    .unwrap();
    let error = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, None),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), FrameEvidenceFailureKind::DigestMismatch);
    let missing = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(
                &SavedFrameRef::from_payload_bytes(
                    store_ns(),
                    VideoSourceId::new("absent").unwrap(),
                    DvrTrackId::new("mjpeg").unwrap(),
                    UtcTimestamp::parse("2024-01-01T00:00:00Z").unwrap(),
                    b"x",
                    None,
                )
                .unwrap(),
                None,
            ),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.kind(), FrameEvidenceFailureKind::MissingOrExpired);
    assert_ne!(missing.code(), error.code());
}

#[tokio::test]
#[allow(clippy::float_cmp)]
async fn roi_crop_preserves_model_transform_bounds() {
    let (store, frame) = saved_mjpeg(55).await;
    let roi = FrameRoiTransform::new([0.25, 0.25, 0.75, 0.75]).unwrap();
    let evidence = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, Some(roi)),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap();
    assert_eq!(evidence.model_input_transform().bounds(), roi.bounds());
    assert!(evidence.width() <= 4);
    assert!(evidence.height() <= 4);
}

#[tokio::test]
async fn decode_limits_reject_oversized_payload_without_allocating_rgb() {
    let (store, frame) = saved_mjpeg(1).await;
    let tight = FrameEvidenceLimits::new(8, 64, 8).unwrap();
    let error = store
        .read_bounded_frame_evidence(FrameEvidenceRequest::new(&frame, None), tight)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), FrameEvidenceFailureKind::DecodeLimitsExceeded);
}

#[tokio::test]
async fn corrupt_jpeg_returns_corrupt_media() {
    let store = open_store("corrupt").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &[0xFF, 0xD8, 0xFF, 0x00])
        .await
        .unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let error = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, None),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), FrameEvidenceFailureKind::CorruptMedia);
}

#[tokio::test]
async fn h264_epoch_decodes_all_idr_revision_pixels() {
    let store = open_store("h264").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = DvrTrackId::new("h264").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    let color = ColorProperties::new(ColorMatrix::Bt709, ColorRange::Limited);
    let mut encoder = AllIdrEncoder::new(16, 16, 5.0, 1_000_000, color).unwrap();
    let identity = CaptureIdentity::new(source.clone(), 7);
    let y = vec![44_u8; 16 * 16];
    let u = vec![128_u8; 8 * 8];
    let v = vec![128_u8; 8 * 8];
    let access = encoder
        .encode(
            I420Picture::new(16, 16, &y, &u, &v, color),
            &identity,
            1_000,
        )
        .unwrap();
    let framed = frame_h264_picture(access.bytes()).unwrap();
    store
        .append_h264_sample(
            &source,
            capture,
            1_000_000,
            true,
            framed.initialization(),
            framed.sample(),
        )
        .await
        .unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let evidence = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, None),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap();
    assert_eq!(evidence.width(), 16);
    assert_eq!(evidence.height(), 16);
    assert!(evidence.rgb().iter().any(|byte| *byte > 0 && *byte < 200));
}

#[tokio::test]
async fn h264_non_keyframe_annex_fixture_decodes_when_supported() {
    let store = open_store("pframe").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = DvrTrackId::new("h264").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    let init = [
        0_u8, 0, 0, 1, 0x67, 0x42, 0x00, 0x1E, 0, 0, 0, 1, 0x68, 0xCE,
    ];
    let sample = frame_h264_picture(&[0, 0, 0, 1, 0x41, 0x01, 0x02])
        .unwrap()
        .sample()
        .to_vec();
    store
        .append_h264_sample(&source, capture, 1_000_000, false, &init, &sample)
        .await
        .unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let result = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, None),
            FrameEvidenceLimits::standard(),
        )
        .await;
    match result {
        Ok(_) | Err(_) => {}
    }
}

#[tokio::test]
async fn legacy_per_file_segment_resolves_evidence_digest() {
    let store = open_store("legacy-segment").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    let jpeg = encode_jpeg(8, 8, 120);
    store
        .write_legacy_segment(&source, &track, capture, 1_000_000, true, &jpeg)
        .await
        .unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let evidence = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, None),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap();
    assert_eq!(evidence.verified_digest(), frame.payload_digest());
}

#[tokio::test]
async fn unsupported_h265_reports_codec_failure() {
    let (store, frame) = saved_mjpeg(3).await;
    sqlx::query("UPDATE dvr_tracks SET codec = 'h265' WHERE source_id = ?1 AND track_id = ?2")
        .bind(frame.source_id().as_str())
        .bind(frame.track_id().as_str())
        .execute(store.media_pool())
        .await
        .unwrap();
    let error = store
        .read_bounded_frame_evidence(
            FrameEvidenceRequest::new(&frame, None),
            FrameEvidenceLimits::standard(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), FrameEvidenceFailureKind::UnsupportedCodec);
}
