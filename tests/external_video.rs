//! Live video from the configured external camera.

use std::process::Command;
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabCommand, A2aLabError, A2aLabResult, A2aLabService, FollowVideoRequest,
    ListVideoSourcesRequest, MemoryLogs, MemoryMetrics, MemoryTasks, PageRequest, VideoProvider,
    VideoSourceId,
};
use a2a_lab_ot2::camera::EXTERNAL_CAMERA_SOURCE_ID;
use a2a_lab_ot2::{LiveJpegSource, VideoServeConfig, open_video_catalog};

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
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
    assert!(on.summary().contains("disk"));
}

#[test]
fn invalid_video_limits_fail_before_robot_contact() {
    for args in [
        vec!["--video-fps", "0"],
        vec!["--video-retention-secs", "0"],
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

#[tokio::test]
async fn injected_external_camera_is_listed_and_followed() {
    let bytes = jpeg(2, 3);
    let config = isolated_config(2, 1024);
    let catalog = open_video_catalog(&config, Some(Box::new(Repeat(bytes.clone()))), None)
        .await
        .unwrap()
        .expect("video on");
    let provider = catalog.provider();
    let mut listed = None;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let page = provider
            .list_video_sources(
                ListVideoSourcesRequest::new(PageRequest::new(None, 10).unwrap()).unwrap(),
            )
            .await
            .unwrap();
        listed = Some(page);
        let follow = provider
            .follow_video(FollowVideoRequest::new(
                VideoSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
            ))
            .await;
        if follow.is_ok() {
            break;
        }
    }
    let page = listed.unwrap();
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
    let bytes = jpeg(4, 4);
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
