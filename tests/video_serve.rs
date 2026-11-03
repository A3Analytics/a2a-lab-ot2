//! Serve-mode dual-source video wiring, retention flags, and in-process gateway.

#![allow(dead_code)]

use std::process::Command;
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabResult, A2aLabService, FollowVideoRequest,
    GetVideoTimelineRequest, ListVideoSourcesRequest, McpLab, McpServer, PageRequest,
    VideoProvider, VideoSourceId, bind_local,
};
use a2a_lab_ot2::camera::CAMERA_SOURCE_ID;
use a2a_lab_ot2::camera::EXTERNAL_CAMERA_SOURCE_ID;
use a2a_lab_ot2::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, ImageServeConfig, LiveJpegSource,
    OpentronsLab, VideoServeConfig, image_catalog, open_video_catalog,
};
use tokio::net::TcpListener;

mod support;

use support::Mock;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
}

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

struct Repeat(Vec<u8>);

impl LiveJpegSource for Repeat {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, a2a_lab_dev_kit::A2aLabError> {
        Ok(self.0.clone())
    }
}

struct FailExternal;

impl LiveJpegSource for FailExternal {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, a2a_lab_dev_kit::A2aLabError> {
        Err(a2a_lab_dev_kit::A2aLabError::unavailable(
            "external pump failed",
        ))
    }
}

fn page() -> PageRequest {
    PageRequest::new(None, 20).unwrap()
}

fn temp_dvr(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-video-serve-{name}-{}",
        std::process::id()
    ))
}

#[test]
fn help_lists_disk_retention_and_dvr_root() {
    let output = bin().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--video-disk-retention-secs"));
    assert!(help.contains("--video-dvr-root"));
    assert!(help.contains(DEFAULT_VIDEO_DVR_ROOT));
}

#[test]
fn invalid_disk_retention_fails_before_robot_contact() {
    let output = bin()
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args(["--video-disk-retention-secs", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("greater than zero"), "{err}");
}

#[test]
fn startup_keeps_videos_off_without_native_sources() {
    let output = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("videos       off"));
    assert!(!out.contains("dvr "));
}

#[test]
fn startup_summary_names_memory_disk_and_dvr_path() {
    let dvr = temp_dvr("startup");
    let output = bin()
        .env("A2ALAB_EXTERNAL_CAMERA_INDEX", "4")
        .env("A2ALAB_VIDEO_RETENTION_SECS", "5")
        .env("A2ALAB_VIDEO_DISK_RETENTION_SECS", "3600")
        .env("A2ALAB_VIDEO_DVR_ROOT", dvr.to_string_lossy().as_ref())
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("external-camera index 4"));
    assert!(out.contains("memory 5s"));
    assert!(out.contains("disk 3600s"));
    assert!(out.contains(&format!("dvr {}", dvr.display())));
}

#[test]
fn camera_only_startup_lists_video_without_external_index() {
    let output = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .args(["--camera-index", "2"])
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("camera index 2"));
    assert!(out.contains("memory 5s"));
    assert!(out.contains(&format!("disk {DEFAULT_DISK_VIDEO_RETENTION_SECS}s")));
}

#[tokio::test]
async fn serve_stack_lists_the_same_sources_on_a2a_and_mcp() {
    let mock = Mock::new();
    let (url, _) = mock.bind().await;
    let image_config = ImageServeConfig::new(Some(4), None, 1024, 1_000, 4).unwrap();
    let video_config =
        VideoServeConfig::with_disk(Some(4), None, 4, 5, 86_400, temp_dvr("stack"), 1024).unwrap();
    let bytes = jpeg(2, 2);
    let videos = open_video_catalog(&video_config, Some(Box::new(Repeat(bytes))), None)
        .await
        .unwrap()
        .expect("videos on");
    let images = image_catalog(&url, &image_config, None).unwrap();
    let lab = OpentronsLab::new(&url).unwrap();
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab)
        .with_images(images)
        .with_image_transport(image_config.transport())
        .with_videos(videos.provider())
        .share();
    let mcp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_addr = mcp_listener.local_addr().unwrap();
    let mcp = McpServer::new(&service);
    tokio::spawn(async move {
        mcp.serve_http(mcp_listener).await.unwrap();
    });
    let mcp_url = format!("http://{mcp_addr}/mcp");
    let mcp_lab = McpLab::connect_with(&mcp_url, image_config.transport())
        .await
        .unwrap();
    let (a2a_listener, a2a_addr) = bind_local().await.unwrap();
    let a2a = a2a_lab_dev_kit::A2aServer::new(&service);
    tokio::spawn(async move {
        a2a.listen(a2a_listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    let client = A2aClient::new(&format!("http://{a2a_addr}")).unwrap();
    wait_for_live(&client, EXTERNAL_CAMERA_SOURCE_ID).await;
    let a2a_page = client
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    let outcome = mcp_lab
        .execute(A2aLabCommand::ListVideoSources(
            ListVideoSourcesRequest::new(page()).unwrap(),
        ))
        .await
        .unwrap();
    let A2aLabResult::ListVideoSources(mcp_page) = outcome.task.result else {
        panic!("list video sources");
    };
    let ids = |page: &a2a_lab_dev_kit::Page<a2a_lab_dev_kit::VideoSource>| {
        page.items()
            .iter()
            .map(|source| source.id().as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&a2a_page), vec![EXTERNAL_CAMERA_SOURCE_ID.to_owned()]);
    assert_eq!(ids(&a2a_page), ids(&mcp_page));
    let timeline = client
        .get_video_timeline(GetVideoTimelineRequest::new(
            VideoSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert!(timeline.newest_at().is_some());
    videos.shutdown();
}

#[tokio::test]
async fn live_gateway_uses_the_in_process_service_not_a_loopback_client() {
    let mock = Mock::new();
    let (url, _) = mock.bind().await;
    let video_config =
        VideoServeConfig::with_disk(Some(4), None, 30, 5, 86_400, temp_dvr("gateway"), 1024)
            .unwrap();
    let bytes = jpeg(3, 3);
    let videos = open_video_catalog(&video_config, Some(Box::new(Repeat(bytes.clone()))), None)
        .await
        .unwrap()
        .unwrap();
    let lab = OpentronsLab::new(&url).unwrap();
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab)
        .with_videos(videos.provider())
        .share();
    let (a2a_listener, a2a_addr) = bind_local().await.unwrap();
    let a2a = a2a_lab_dev_kit::A2aServer::new(&service);
    tokio::spawn(async move {
        a2a.listen(a2a_listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    let client = A2aClient::new(&format!("http://{a2a_addr}")).unwrap();
    wait_for_live(&client, EXTERNAL_CAMERA_SOURCE_ID).await;
    let follow = client
        .follow_video(FollowVideoRequest::new(
            VideoSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert!(follow.path().starts_with("/media/video-live/"));
    let http = reqwest::Client::new();
    let response = http
        .get(format!("http://{a2a_addr}{}", follow.path()))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    videos.shutdown();
}

#[tokio::test]
async fn one_live_pump_failure_does_not_remove_the_other_source() {
    let video_config =
        VideoServeConfig::with_disk(Some(4), None, 30, 5, 86_400, temp_dvr("isolation"), 1024)
            .unwrap()
            .with_camera_index(1);
    let camera_bytes = jpeg(4, 4);
    let videos = open_video_catalog(
        &video_config,
        Some(Box::new(FailExternal)),
        Some(Box::new(Repeat(camera_bytes))),
    )
    .await
    .unwrap()
    .unwrap();
    let provider = videos.provider();
    wait_for_live_provider(&provider, CAMERA_SOURCE_ID).await;
    let page = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    videos.shutdown();
}

async fn wait_for_live(client: &A2aClient, source_id: &str) {
    let source = VideoSourceId::new(source_id).unwrap();
    for _ in 0..50 {
        if client
            .follow_video(FollowVideoRequest::new(source.clone()))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("live follow never became available for {source_id}");
}

async fn wait_for_live_provider(provider: &a2a_lab_ot2::VideoDvrCatalog, source_id: &str) {
    let source = VideoSourceId::new(source_id).unwrap();
    for _ in 0..50 {
        if provider
            .follow_video(FollowVideoRequest::new(source.clone()))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("live follow never became available for {source_id}");
}
