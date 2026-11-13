//! Hardware-free external vs webcam media contract matrix.

#![allow(clippy::large_futures)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabError, A2aLabResult, A2aLabService, A2aServer,
    DvrVideoProvider, FollowVideoRequest, GetCurrentImageRequest, GetVideoClipRequest,
    GetVideoFrameAtRequest, GetVideoTimelineRequest, HttpAuthSecurityScheme, ImageProvider,
    ListVideoSourcesRequest, McpLab, McpServer, PageRequest, SecurityScheme, TimeRange,
    UtcTimestamp, VideoProvider, VideoSourceId, bind_local, video_clip_path,
};
use a2a_lab_ot2::camera::{
    CAMERA_SOURCE_ID, CameraCapture, CameraCaptureError, EXTERNAL_CAMERA_HEIGHT,
    EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH, ExternalCameraFormat, remember_camera_frame,
    remember_external_frame,
};
use a2a_lab_ot2::{
    DeclarativeVideoServe, ImageServeConfig, LiveImageCatalog, LiveJpegSource, LiveVideoInjections,
    OpentronsLab, VideoServeConfig, image_catalog, open_video_catalog,
};
use tokio::net::TcpListener;

pub(crate) const INTENTIONAL_MEDIA_PARITY_DIFFERENCES: &str = "
intentional external vs camera media differences:
- source ids are external-camera and camera; descriptions name the native camera index vs the second webcam.
- hardware selection uses A2ALAB_EXTERNAL_CAMERA_INDEX / --external-camera-index vs A2ALAB_CAMERA_INDEX / --camera-index.
- when declarative video is running, both sources read stills from the shared warm-still cache; encoded sizes follow each source's configured resolution.
- diagnostic text names Nokhwa for the external camera; shared contract uses the same error code classes and metadata fields.
";

const EXT: &str = EXTERNAL_CAMERA_SOURCE_ID;
const CAM: &str = CAMERA_SOURCE_ID;

struct Endpoints {
    a2a: A2aClient,
    mcp: McpLab,
}

struct Lab {
    videos: DeclarativeVideoServe,
    images: LiveImageCatalog,
    provider: a2a_lab_dev_kit::DvrVideoProvider,
    endpoints: Endpoints,
    a2a_base: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ContractObs {
    Ok,
    Code(String),
    Count(usize),
    Bool(bool),
    Text(String),
}

fn jpeg(width: u16, height: u16, marker: u8) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    let image: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_fn(u32::from(width), u32::from(height), |_, _| {
            Rgb([marker, marker, marker])
        });
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 80)
        .write_image(
            image.as_raw(),
            u32::from(width),
            u32::from(height),
            ExtendedColorType::Rgb8,
        )
        .expect("jpeg");
    out
}

fn external_live_jpeg(marker: u8) -> Vec<u8> {
    jpeg(
        u16::try_from(EXTERNAL_CAMERA_WIDTH).expect("width"),
        u16::try_from(EXTERNAL_CAMERA_HEIGHT).expect("height"),
        marker,
    )
}

fn camera_live_jpeg(marker: u8) -> Vec<u8> {
    jpeg(1280, 720, marker)
}

fn page() -> PageRequest {
    PageRequest::new(None, 20).unwrap()
}

fn video_id(source: &str) -> VideoSourceId {
    VideoSourceId::new(source).unwrap()
}

fn temp_dvr(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-media-parity-{name}-{}",
        std::process::id()
    ))
}

fn mismatch(
    interface: &str,
    case: &str,
    source: &str,
    expected: &ContractObs,
    actual: &ContractObs,
) -> ! {
    panic!(
        "media parity mismatch\nsource: {source}\ninterface: {interface}\ncase: {case}\nexpected: {expected:?}\nactual: {actual:?}"
    );
}

#[allow(clippy::needless_pass_by_value)]
fn compare_sources(interface: &str, case: &str, ext: ContractObs, orb: ContractObs) {
    if ext != orb {
        mismatch(interface, case, "external-camera vs camera", &ext, &orb);
    }
}

#[allow(clippy::needless_pass_by_value)]
fn compare_a2a_mcp(case: &str, a2a: ContractObs, mcp: ContractObs) {
    if a2a != mcp {
        mismatch("A2A vs MCP", case, "both sources", &a2a, &mcp);
    }
}

struct Remembering {
    bytes: Vec<u8>,
    side: Side,
}

enum Side {
    External,
    Webcam,
}

impl LiveJpegSource for Remembering {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        match self.side {
            Side::External => remember_external_frame(self.bytes.clone()),
            Side::Webcam => remember_camera_frame(self.bytes.clone()),
        }
        Ok(self.bytes.clone())
    }
}

struct Advancing {
    step: AtomicUsize,
    side: Side,
}

impl LiveJpegSource for Advancing {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        let n = self.step.fetch_add(1, Ordering::SeqCst);
        let marker = u8::try_from(n % 250).unwrap_or(1);
        let bytes = match self.side {
            Side::External => external_live_jpeg(marker),
            Side::Webcam => camera_live_jpeg(marker),
        };
        match self.side {
            Side::External => remember_external_frame(bytes.clone()),
            Side::Webcam => remember_camera_frame(bytes.clone()),
        }
        Ok(bytes)
    }
}

struct FailLive;

impl LiveJpegSource for FailLive {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        Err(A2aLabError::unavailable("injected live pump unavailable"))
    }
}

struct BadJpeg;

impl LiveJpegSource for BadJpeg {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        Err(A2aLabError::invalid("jpeg", "malformed injected frame"))
    }
}

struct FlakyLive {
    left: Mutex<usize>,
    bytes: Vec<u8>,
    side: Side,
}

impl LiveJpegSource for FlakyLive {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        let mut left = self.left.lock().expect("failures");
        if *left > 0 {
            *left -= 1;
            return Err(A2aLabError::unavailable("injected disconnect"));
        }
        match self.side {
            Side::External => remember_external_frame(self.bytes.clone()),
            Side::Webcam => remember_camera_frame(self.bytes.clone()),
        }
        Ok(self.bytes.clone())
    }
}

struct CaptureOnce(Arc<AtomicUsize>, Vec<u8>);

impl CameraCapture for CaptureOnce {
    fn capture(
        &self,
        _index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(self.1.clone())
    }
}

async fn wait_for_follow(provider: &DvrVideoProvider, source: &str) {
    let id = video_id(source);
    for _ in 0..400 {
        if provider.live_h264().get(&id).is_some()
            && provider
                .follow_video(FollowVideoRequest::new(id.clone()))
                .await
                .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("live follow never became available for {source}");
}

async fn open_lab(dvr_root: &Path, mut injections: LiveVideoInjections) -> Lab {
    let _ = std::fs::remove_dir_all(dvr_root);
    let video_config = VideoServeConfig::with_disk(
        Some(4),
        None,
        8,
        5,
        86_400,
        dvr_root.to_path_buf(),
        16_777_216,
    )
    .unwrap()
    .with_camera_index(1);
    let external = injections.jpeg_sources.remove(EXTERNAL_CAMERA_SOURCE_ID);
    let webcam = injections.jpeg_sources.remove(CAMERA_SOURCE_ID);
    let videos = open_video_catalog(&video_config, external, webcam)
        .await
        .unwrap()
        .expect("videos on");
    let provider = videos.provider();
    wait_for_follow(&provider, EXT).await;
    wait_for_follow(&provider, CAM).await;
    for _ in 0..200 {
        if videos.still_cache().latest(EXT).is_some() && videos.still_cache().latest(CAM).is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let image_config = ImageServeConfig::new(Some(4), None, 16_777_216, 100, 4)
        .unwrap()
        .with_camera_index(1);
    let ext_still = CaptureOnce(Arc::new(AtomicUsize::new(0)), jpeg(1, 1, 9));
    let images = image_catalog(
        "http://127.0.0.1:9",
        &image_config,
        Some(Arc::new(ext_still)),
        None,
        Some(videos.still_cache()),
    )
    .unwrap();

    let lab = OpentronsLab::new("http://127.0.0.1:9").unwrap();
    let transport = image_config.transport();
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab)
        .with_images(images.clone())
        .with_image_transport(transport)
        .with_videos(provider.clone())
        .share();
    let mcp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_addr = mcp_listener.local_addr().unwrap();
    let mcp_server = McpServer::new(&service);
    tokio::spawn(async move {
        mcp_server.serve_http(mcp_listener).await.unwrap();
    });
    let (a2a_listener, a2a_addr) = bind_local().await.unwrap();
    let a2a_server = A2aServer::new(&service);
    tokio::spawn(async move {
        a2a_server.listen(a2a_listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    let a2a_base = format!("http://{a2a_addr}");
    let endpoints = Endpoints {
        a2a: A2aClient::new(&a2a_base).unwrap(),
        mcp: McpLab::connect_with(&format!("http://{mcp_addr}/mcp"), transport)
            .await
            .unwrap(),
    };
    Lab {
        videos,
        images,
        provider,
        endpoints,
        a2a_base,
    }
}

async fn secured_live_gateway(provider: a2a_lab_dev_kit::DvrVideoProvider) {
    let lab = OpentronsLab::new("http://127.0.0.1:9").unwrap();
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab)
        .with_videos(provider)
        .share();
    let schemes = [(
        "bearerAuth".to_owned(),
        SecurityScheme::HttpAuth(HttpAuthSecurityScheme {
            scheme: "bearer".to_owned(),
            description: None,
            bearer_format: None,
        }),
    )]
    .into();
    let requirements = vec![[("bearerAuth".to_owned(), Vec::new())].into()];
    let (listener, address) = bind_local().await.unwrap();
    let base = format!("http://{address}");
    tokio::spawn(async move {
        A2aServer::new(&service)
            .with_security(schemes, requirements)
            .listen(listener)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    let authed = A2aClient::new(&base)
        .unwrap()
        .with_bearer_token("lab-token");
    let source = video_id(EXT);
    for _ in 0..120 {
        if authed
            .follow_video(FollowVideoRequest::new(source.clone()))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let follow = {
        let mut ready = None;
        for _ in 0..200 {
            if let Ok(follow) = authed
                .follow_video(FollowVideoRequest::new(video_id(EXT).clone()))
                .await
            {
                ready = Some(follow);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        ready.unwrap_or_else(|| panic!("live follow never became available for {EXT}"))
    };
    let ok = reqwest::Client::new()
        .get(format!("{base}{}", follow.path()))
        .header("Authorization", "Bearer lab-token")
        .send()
        .await
        .unwrap();
    assert!(ok.status().is_success(), "{}", ok.status());
    let denied = reqwest::Client::new()
        .get(format!("{base}{}", follow.path()))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
}

async fn provider_list_contains(provider: &impl VideoProvider, source: &str) -> ContractObs {
    match provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
    {
        Ok(page) => {
            let found = page.items().iter().any(|item| item.id().as_str() == source);
            ContractObs::Bool(found)
        }
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn provider_follow(provider: &impl VideoProvider, source: &str) -> ContractObs {
    match provider
        .follow_video(FollowVideoRequest::new(video_id(source)))
        .await
    {
        Ok(follow) if follow.path().starts_with("/media/video-live/") => ContractObs::Ok,
        Ok(_) => ContractObs::Text("follow missing live path".to_owned()),
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn provider_timeline(provider: &impl VideoProvider, source: &str) -> ContractObs {
    match provider
        .get_video_timeline(GetVideoTimelineRequest::new(video_id(source)))
        .await
    {
        Ok(timeline) => ContractObs::Bool(timeline.newest_at().is_some()),
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn provider_frame_at(
    provider: &impl VideoProvider,
    source: &str,
    at: UtcTimestamp,
) -> ContractObs {
    match provider
        .get_video_frame_at(GetVideoFrameAtRequest::new(video_id(source), at, None))
        .await
    {
        Ok(_) => ContractObs::Ok,
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn provider_clip(
    provider: &impl VideoProvider,
    source: &str,
    start: &str,
    end: &str,
) -> ContractObs {
    let range = TimeRange::new(
        UtcTimestamp::parse(start).unwrap(),
        UtcTimestamp::parse(end).unwrap(),
    )
    .unwrap();
    match provider
        .get_video_clip(GetVideoClipRequest::new(video_id(source), range).unwrap())
        .await
    {
        Ok(clip) if clip.path() == video_clip_path(clip.id()) => ContractObs::Ok,
        Ok(_) => ContractObs::Text("clip path".to_owned()),
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn a2a_list(endpoints: &Endpoints, source: &str) -> ContractObs {
    match endpoints
        .a2a
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
    {
        Ok(page) => ContractObs::Bool(page.items().iter().any(|item| item.id().as_str() == source)),
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn mcp_list(endpoints: &Endpoints, source: &str) -> ContractObs {
    match endpoints
        .mcp
        .execute(A2aLabCommand::ListVideoSources(
            ListVideoSourcesRequest::new(page()).unwrap(),
        ))
        .await
    {
        Ok(outcome) => match outcome.task.result {
            A2aLabResult::ListVideoSources(page) => {
                ContractObs::Bool(page.items().iter().any(|item| item.id().as_str() == source))
            }
            _ => ContractObs::Text("protocol".to_owned()),
        },
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn a2a_follow(endpoints: &Endpoints, source: &str) -> ContractObs {
    match endpoints
        .a2a
        .follow_video(FollowVideoRequest::new(video_id(source)))
        .await
    {
        Ok(follow) if follow.path().starts_with("/media/video-live/") => ContractObs::Ok,
        Ok(_) => ContractObs::Text("follow path".to_owned()),
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn mcp_follow(endpoints: &Endpoints, source: &str) -> ContractObs {
    match endpoints
        .mcp
        .execute(A2aLabCommand::FollowVideo(FollowVideoRequest::new(
            video_id(source),
        )))
        .await
    {
        Ok(outcome) => match outcome.task.result {
            A2aLabResult::FollowVideo(follow)
                if follow.path().starts_with("/media/video-live/") =>
            {
                ContractObs::Ok
            }
            _ => ContractObs::Text("follow protocol".to_owned()),
        },
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn still_bytes(images: &LiveImageCatalog, source: &str) -> ContractObs {
    let id = a2a_lab_dev_kit::ImageSourceId::new(source).unwrap();
    match images
        .get_current_image(GetCurrentImageRequest::new(id))
        .await
    {
        Ok(image) => ContractObs::Count(image.data().len()),
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn compare_still_live(lab: &Lab, case: &str) {
    let ext = still_bytes(&lab.images, EXT).await;
    let orb = still_bytes(&lab.images, CAM).await;
    assert!(matches!(ext, ContractObs::Count(len) if len > 0));
    assert!(matches!(orb, ContractObs::Count(len) if len > 0));
    compare_a2a_mcp(
        case,
        a2a_still(&lab.endpoints, EXT).await,
        mcp_still(&lab.endpoints, EXT).await,
    );
    compare_a2a_mcp(
        case,
        a2a_still(&lab.endpoints, CAM).await,
        mcp_still(&lab.endpoints, CAM).await,
    );
}

async fn a2a_still(endpoints: &Endpoints, source: &str) -> ContractObs {
    let id = a2a_lab_dev_kit::ImageSourceId::new(source).unwrap();
    match endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(id))
        .await
    {
        Ok(image) => ContractObs::Count(image.data().len()),
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn mcp_still(endpoints: &Endpoints, source: &str) -> ContractObs {
    let id = a2a_lab_dev_kit::ImageSourceId::new(source).unwrap();
    match endpoints
        .mcp
        .execute(A2aLabCommand::GetCurrentImage(GetCurrentImageRequest::new(
            id,
        )))
        .await
    {
        Ok(outcome) => match outcome.task.result {
            A2aLabResult::GetCurrentImage(image) => ContractObs::Count(image.data().len()),
            _ => ContractObs::Text("still protocol".to_owned()),
        },
        Err(error) => ContractObs::Code(error.code().to_owned()),
    }
}

async fn compare_interface_parity(lab: &Lab, case: &str, source: &str) {
    let provider = provider_follow(&lab.provider, source).await;
    let a2a = a2a_follow(&lab.endpoints, source).await;
    let mcp = mcp_follow(&lab.endpoints, source).await;
    compare_a2a_mcp(case, a2a.clone(), mcp);
    if provider != ContractObs::Ok {
        mismatch("provider", case, source, &ContractObs::Ok, &provider);
    }
    if a2a != ContractObs::Ok {
        mismatch("A2A", case, source, &ContractObs::Ok, &a2a);
    }
}

#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
pub async fn run() {
    let dvr = temp_dvr("matrix");
    let ext_live = external_live_jpeg(20);
    let orb_live = camera_live_jpeg(21);
    let lab = open_lab(
        &dvr,
        LiveVideoInjections::legacy_jpeg(
            Some(Box::new(Remembering {
                bytes: ext_live.clone(),
                side: Side::External,
            })),
            Some(Box::new(Remembering {
                bytes: orb_live.clone(),
                side: Side::Webcam,
            })),
        ),
    )
    .await;

    // Discovery
    compare_sources(
        "provider",
        "video source discovery",
        provider_list_contains(&lab.provider, EXT).await,
        provider_list_contains(&lab.provider, CAM).await,
    );
    compare_sources(
        "A2A",
        "video source discovery",
        a2a_list(&lab.endpoints, EXT).await,
        a2a_list(&lab.endpoints, CAM).await,
    );
    compare_a2a_mcp(
        "video source discovery external",
        a2a_list(&lab.endpoints, EXT).await,
        mcp_list(&lab.endpoints, EXT).await,
    );
    compare_a2a_mcp(
        "video source discovery camera",
        a2a_list(&lab.endpoints, CAM).await,
        mcp_list(&lab.endpoints, CAM).await,
    );

    compare_still_live(&lab, "still/live JPEG reuse").await;
    compare_interface_parity(&lab, "live follow", EXT).await;
    compare_interface_parity(&lab, "live follow", CAM).await;
    secured_live_gateway(lab.provider.clone()).await;
    let follow_open = lab
        .endpoints
        .a2a
        .follow_video(FollowVideoRequest::new(video_id(CAM)))
        .await
        .unwrap();
    let open = reqwest::Client::new()
        .get(format!("{}{}", lab.a2a_base, follow_open.path()))
        .send()
        .await
        .unwrap();
    assert!(open.status().is_success(), "{}", open.status());

    // Advancing frames
    lab.videos.shutdown();
    let dvr_adv = temp_dvr("advancing");
    let advancing = open_lab(
        &dvr_adv,
        LiveVideoInjections::legacy_jpeg(
            Some(Box::new(Advancing {
                step: AtomicUsize::new(0),
                side: Side::External,
            })),
            Some(Box::new(Advancing {
                step: AtomicUsize::new(0),
                side: Side::Webcam,
            })),
        ),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    compare_sources(
        "provider",
        "advancing frames timeline",
        provider_timeline(&advancing.provider, EXT).await,
        provider_timeline(&advancing.provider, CAM).await,
    );

    // Half-open clip bounds (end excluded) via no_recording when empty range miss
    let clip_ext = provider_clip(
        &advancing.provider,
        EXT,
        "2099-01-01T00:00:00Z",
        "2099-01-01T00:00:01Z",
    )
    .await;
    let clip_orb = provider_clip(
        &advancing.provider,
        CAM,
        "2099-01-01T00:00:00Z",
        "2099-01-01T00:00:01Z",
    )
    .await;
    compare_sources("provider", "clip outside coverage", clip_ext, clip_orb);

    advancing.videos.shutdown();

    // Five-second live memory window (configured in open_lab)
    let dvr_live = temp_dvr("live-window");
    let live_lab = open_lab(
        &dvr_live,
        LiveVideoInjections::legacy_jpeg(
            Some(Box::new(Remembering {
                bytes: external_live_jpeg(30),
                side: Side::External,
            })),
            Some(Box::new(Remembering {
                bytes: camera_live_jpeg(31),
                side: Side::Webcam,
            })),
        ),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(5_500)).await;
    compare_sources(
        "provider",
        "five-second live retention still follows",
        provider_follow(&live_lab.provider, EXT).await,
        provider_follow(&live_lab.provider, CAM).await,
    );
    live_lab.videos.shutdown();

    // Restart recovery on shared DVR root
    let dvr_restart = temp_dvr("restart");
    {
        let first = open_lab(
            &dvr_restart,
            LiveVideoInjections::legacy_jpeg(
                Some(Box::new(Remembering {
                    bytes: external_live_jpeg(40),
                    side: Side::External,
                })),
                Some(Box::new(Remembering {
                    bytes: camera_live_jpeg(41),
                    side: Side::Webcam,
                })),
            ),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        first.videos.shutdown();
    }
    let second = open_lab(
        &dvr_restart,
        LiveVideoInjections::legacy_jpeg(
            Some(Box::new(Remembering {
                bytes: external_live_jpeg(42),
                side: Side::External,
            })),
            Some(Box::new(Remembering {
                bytes: camera_live_jpeg(43),
                side: Side::Webcam,
            })),
        ),
    )
    .await;
    compare_sources(
        "provider",
        "restart recovery timeline",
        provider_timeline(&second.provider, EXT).await,
        provider_timeline(&second.provider, CAM).await,
    );
    second.videos.shutdown();

    // Genuine recording gap via indexed DVR append
    let dvr_gap = temp_dvr("gap");
    let gap_lab = open_lab(
        &dvr_gap,
        LiveVideoInjections::legacy_jpeg(
            Some(Box::new(Remembering {
                bytes: external_live_jpeg(50),
                side: Side::External,
            })),
            Some(Box::new(Remembering {
                bytes: camera_live_jpeg(51),
                side: Side::Webcam,
            })),
        ),
    )
    .await;
    let store = gap_lab.videos.indexed_dvr().clone();
    gap_lab.videos.shutdown();
    for source in [EXT, CAM] {
        let id = video_id(source);
        store
            .append_mjpeg_frame(
                &id,
                UtcTimestamp::parse("2024-06-01T00:00:00Z").unwrap(),
                1_000_000,
                &jpeg(2, 2, 52),
            )
            .await
            .unwrap();
        store
            .append_mjpeg_frame(
                &id,
                UtcTimestamp::parse("2024-06-01T00:00:04Z").unwrap(),
                1_000_000,
                &jpeg(2, 2, 53),
            )
            .await
            .unwrap();
    }
    let gap_ext = provider_frame_at(
        &gap_lab.provider,
        EXT,
        UtcTimestamp::parse("2024-06-01T00:00:02Z").unwrap(),
    )
    .await;
    let gap_orb = provider_frame_at(
        &gap_lab.provider,
        CAM,
        UtcTimestamp::parse("2024-06-01T00:00:02Z").unwrap(),
    )
    .await;
    compare_sources(
        "provider",
        "recording gap frame-at",
        gap_ext.clone(),
        gap_orb,
    );
    assert_eq!(gap_ext, ContractObs::Code("recording_gap".to_owned()));
    let half_ext = provider_clip(
        &gap_lab.provider,
        EXT,
        "2024-06-01T00:00:00Z",
        "2024-06-01T00:00:02Z",
    )
    .await;
    let half_orb = provider_clip(
        &gap_lab.provider,
        CAM,
        "2024-06-01T00:00:00Z",
        "2024-06-01T00:00:02Z",
    )
    .await;
    compare_sources("provider", "half-open clip bounds", half_ext, half_orb);

    let outside_ext = provider_frame_at(
        &gap_lab.provider,
        EXT,
        UtcTimestamp::parse("2019-01-01T00:00:00Z").unwrap(),
    )
    .await;
    let outside_orb = provider_frame_at(
        &gap_lab.provider,
        CAM,
        UtcTimestamp::parse("2019-01-01T00:00:00Z").unwrap(),
    )
    .await;
    compare_sources(
        "provider",
        "outside disk retention",
        outside_ext,
        outside_orb,
    );
    gap_lab.videos.shutdown();

    // Unknown source
    let unknown = provider_frame_at(
        &lab.provider,
        "missing-camera",
        UtcTimestamp::parse("2024-06-01T00:00:00Z").unwrap(),
    )
    .await;
    assert_eq!(unknown, ContractObs::Code("not_found".to_owned()));

    // Failure isolation and hardware-free failures
    Box::pin(run_failure_matrix()).await;
}

#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
async fn run_failure_matrix() {
    let dvr = temp_dvr("failures");
    let config = VideoServeConfig::with_disk(Some(4), None, 8, 5, 86_400, dvr.clone(), 16_777_216)
        .unwrap()
        .with_camera_index(1);

    // Unavailable hardware (both live pumps fail)
    let down = open_video_catalog(&config, Some(Box::new(FailLive)), Some(Box::new(FailLive)))
        .await
        .unwrap()
        .expect("videos");
    let ext_down = provider_follow(&down.provider(), EXT).await;
    let orb_down = provider_follow(&down.provider(), CAM).await;
    compare_sources("provider", "unavailable hardware", ext_down, orb_down);
    down.shutdown();

    // One source failed, other still live
    let split = open_video_catalog(
        &config,
        Some(Box::new(FailLive)),
        Some(Box::new(Remembering {
            bytes: camera_live_jpeg(60),
            side: Side::Webcam,
        })),
    )
    .await
    .unwrap()
    .expect("videos");
    wait_for_follow(&split.provider(), CAM).await;
    let ext_follow = provider_follow(&split.provider(), EXT).await;
    let orb_follow = provider_follow(&split.provider(), CAM).await;
    assert_ne!(ext_follow, ContractObs::Ok);
    assert_eq!(orb_follow, ContractObs::Ok);
    split.shutdown();

    // Malformed JPEG on live pump
    let bad = open_video_catalog(&config, Some(Box::new(BadJpeg)), Some(Box::new(BadJpeg)))
        .await
        .unwrap()
        .expect("videos");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let bad_ext = provider_follow(&bad.provider(), EXT).await;
    let bad_orb = provider_follow(&bad.provider(), CAM).await;
    compare_sources("provider", "malformed live jpeg", bad_ext, bad_orb);
    bad.shutdown();

    // Disconnect exhaustion then recovery
    let flaky = open_video_catalog(
        &config,
        Some(Box::new(FlakyLive {
            left: Mutex::new(20),
            bytes: external_live_jpeg(70),
            side: Side::External,
        })),
        Some(Box::new(FlakyLive {
            left: Mutex::new(20),
            bytes: camera_live_jpeg(71),
            side: Side::Webcam,
        })),
    )
    .await
    .unwrap()
    .expect("videos");
    wait_for_follow(&flaky.provider(), EXT).await;
    wait_for_follow(&flaky.provider(), CAM).await;
    flaky.shutdown();

    // Expired clip session
    let store = a2a_lab_ot2::IndexedVideoDvr::open(&temp_dvr("clip-expire"), 86_400)
        .await
        .unwrap();
    let live = a2a_lab_dev_kit::MemoryVideos::new();
    let catalog =
        a2a_lab_ot2::VideoDvrCatalog::open(live, store, Duration::from_millis(1)).unwrap();
    let source = video_id(EXT);
    catalog
        .dvr()
        .append_mjpeg_frame(
            &source,
            UtcTimestamp::parse("2024-06-01T00:00:00Z").unwrap(),
            1_000_000,
            &jpeg(2, 2, 80),
        )
        .await
        .unwrap();
    let range = TimeRange::new(
        UtcTimestamp::parse("2024-06-01T00:00:00Z").unwrap(),
        UtcTimestamp::parse("2024-06-01T00:00:02Z").unwrap(),
    )
    .unwrap();
    let clip = catalog
        .get_video_clip(GetVideoClipRequest::new(source.clone(), range).unwrap())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    match catalog.open_video_clip(clip.id()).await {
        Err(error) => assert_eq!(error.code(), "clip_expired"),
        Ok(_) => panic!("expected expired clip"),
    }

    // Source switching: operate external then camera with same error class
    let switch = open_video_catalog(
        &config,
        Some(Box::new(Remembering {
            bytes: external_live_jpeg(90),
            side: Side::External,
        })),
        Some(Box::new(Remembering {
            bytes: camera_live_jpeg(91),
            side: Side::Webcam,
        })),
    )
    .await
    .unwrap()
    .expect("videos");
    wait_for_follow(&switch.provider(), EXT).await;
    wait_for_follow(&switch.provider(), CAM).await;
    compare_sources(
        "provider",
        "source switching follow",
        provider_follow(&switch.provider(), EXT).await,
        provider_follow(&switch.provider(), CAM).await,
    );
    switch.shutdown();

    let _ = std::fs::remove_dir_all(&dvr);
}
