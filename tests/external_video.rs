//! Live video from the configured external camera.

use std::process::Command;
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabCommand, A2aLabError, A2aLabResult, A2aLabService, DvrVideoProvider, FollowVideoRequest,
    ListVideoSourcesRequest, MemoryLogs, MemoryMetrics, MemoryTasks, PageRequest, VideoProvider,
    VideoSourceId,
};
use a2a_lab_ot2::camera::{
    EXTERNAL_CAMERA_HEIGHT, EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH,
};
use a2a_lab_ot2::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, LiveJpegSource, VideoServeConfig, open_video_catalog,
};

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    let image: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_fn(width, height, |_, _| Rgb([40, 40, 40]));
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 80)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgb8)
        .expect("jpeg");
    out
}

struct Repeat(Vec<u8>);

impl LiveJpegSource for Repeat {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        Ok(self.0.clone())
    }
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
}

#[test]
fn help_lists_video_defaults() {
    let output = bin().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--video-fps"));
    assert!(help.contains("--video-retention-secs"));
    assert!(help.contains("--video-disk-retention-secs"));
    assert!(help.contains("--video-dvr-root"));
    assert!(help.contains(&a2a_lab_ot2::DEFAULT_VIDEO_RETENTION_SECS.to_string()));
}

#[test]
fn config_rejects_zero_and_empty_description() {
    assert_eq!(
        VideoServeConfig::new(Some(4), None, 0, 120, 1024)
            .unwrap_err()
            .code(),
        "invalid"
    );
    assert_eq!(
        VideoServeConfig::new(Some(4), None, 4, 0, 1024)
            .unwrap_err()
            .code(),
        "invalid"
    );
    assert_eq!(
        VideoServeConfig::new(Some(4), Some(""), 4, 120, 1024)
            .unwrap_err()
            .code(),
        "invalid"
    );
}

#[test]
fn summary_tracks_the_external_index() {
    let off = VideoServeConfig::new(None, None, 4, 120, 1024).unwrap();
    assert_eq!(off.summary(), "videos       off");
    let on = VideoServeConfig::new(Some(4), None, 4, 120, 1024).unwrap();
    assert!(on.summary().contains("external-camera index 4"));
    assert!(on.summary().contains("memory 120s"));
    assert!(
        on.summary()
            .contains(&format!("disk {DEFAULT_DISK_VIDEO_RETENTION_SECS}s"))
    );
    assert!(on.summary().contains("dvr "));
}

#[test]
fn invalid_video_limits_fail_before_robot_contact() {
    for args in [
        vec!["--video-fps", "0"],
        vec!["--video-retention-secs", "0"],
        vec!["--video-disk-retention-secs", "0"],
    ] {
        let output = bin()
            .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
            .args(args.clone())
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(err.contains("greater than zero"), "{args:?}: {err}");
        assert!(!err.contains("unreachable"), "{err}");
    }
}

fn isolated_config(live_retention_secs: u64, max_bytes: u64) -> VideoServeConfig {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-video-serve-{}-{}",
        std::process::id(),
        live_retention_secs
    ));
    VideoServeConfig::with_disk(
        Some(4),
        None,
        4,
        live_retention_secs,
        86_400,
        dir,
        max_bytes,
    )
    .unwrap()
}

async fn wait_for_live_follow(provider: &DvrVideoProvider, id: &str) {
    let source = VideoSourceId::new(id).unwrap();
    for _ in 0..200 {
        if provider.live_h264().get(&source).is_some()
            && provider
                .follow_video(FollowVideoRequest::new(source.clone()))
                .await
                .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("live follow never became available for {id}");
}

#[tokio::test]
async fn injected_external_camera_is_listed_and_followed() {
    let bytes = jpeg(EXTERNAL_CAMERA_WIDTH, EXTERNAL_CAMERA_HEIGHT);
    let config = isolated_config(2, 1024);
    let catalog = open_video_catalog(&config, Some(Box::new(Repeat(bytes.clone()))), None)
        .await
        .unwrap()
        .expect("video on");
    let provider = catalog.provider();
    wait_for_live_follow(&provider, EXTERNAL_CAMERA_SOURCE_ID).await;
    let page = provider
        .list_video_sources(
            ListVideoSourcesRequest::new(PageRequest::new(None, 10).unwrap()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items().len(), 1);
    assert_eq!(page.items()[0].id().as_str(), EXTERNAL_CAMERA_SOURCE_ID);
    provider
        .follow_video(FollowVideoRequest::new(
            VideoSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    catalog.shutdown();
}

#[tokio::test]
async fn video_skills_appear_on_the_shared_service() {
    let bytes = jpeg(EXTERNAL_CAMERA_WIDTH, EXTERNAL_CAMERA_HEIGHT);
    let config = isolated_config(2, 1024);
    let catalog = open_video_catalog(&config, Some(Box::new(Repeat(bytes.clone()))), None)
        .await
        .unwrap()
        .unwrap();
    let service = A2aLabService::new(MemoryLogs::new(), MemoryMetrics::new(), MemoryTasks::new())
        .with_videos(catalog.provider())
        .share();
    let outcome = service
        .execute(A2aLabCommand::ListVideoSources(
            ListVideoSourcesRequest::new(PageRequest::new(None, 10).unwrap()).unwrap(),
        ))
        .await
        .unwrap();
    let A2aLabResult::ListVideoSources(sources) = outcome.task.result else {
        panic!("expected list video sources");
    };
    assert_eq!(sources.items()[0].id().as_str(), EXTERNAL_CAMERA_SOURCE_ID);
    catalog.shutdown();
}
