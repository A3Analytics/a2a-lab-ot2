//! Hardware-independent camera demo.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use base64::Engine;
use tokio::net::TcpListener;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
}

fn jpeg() -> Vec<u8> {
    let mut data = vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x02, 0x00, 0x03,
    ];
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

async fn picture_server(bytes: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/camera/picture", post(picture))
        .with_state((Arc::new(bytes), Arc::clone(&hits)));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), hits)
}

async fn picture(State((bytes, hits)): State<(Arc<Vec<u8>>, Arc<AtomicUsize>)>) -> Response {
    hits.fetch_add(1, Ordering::SeqCst);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(bytes.as_ref().clone()))
        .unwrap();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    response
}

#[test]
fn list_image_sources_adds_the_extra_camera_for_the_chosen_index() {
    let output = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .arg("list-image-sources")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("a2a-lab list_image_sources"));
    assert!(text.contains("opentrons-camera"));
    assert!(text.contains("opentrons-ot2"));
    assert!(!text.contains("external-camera"));
    assert!(!text.contains("image/jpeg"));

    let enabled = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .env_remove("A2ALAB_EXTERNAL_CAMERA_DESCRIPTION")
        .args(["--external-camera-index", "9", "list-image-sources"])
        .output()
        .unwrap();
    assert!(enabled.status.success());
    let enabled_text = String::from_utf8(enabled.stdout).unwrap();
    assert!(enabled_text.contains("external-camera"));
    assert!(enabled_text.contains("opentrons-camera"));
    assert!(enabled_text.contains("description\tStill frame from the configured external camera."));

    let named = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .env_remove("A2ALAB_EXTERNAL_CAMERA_DESCRIPTION")
        .args([
            "--external-camera-index",
            "9",
            "--external-camera-description",
            "Bench camera over the deck",
            "list-image-sources",
        ])
        .output()
        .unwrap();
    assert!(named.status.success());
    let named_text = String::from_utf8(named.stdout).unwrap();
    assert!(named_text.contains("description\tBench camera over the deck"));
    assert!(!named_text.contains("Still frame from the configured external camera."));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_saves_a_mock_frame_without_printing_bytes() {
    let bytes = jpeg();
    let (url, hits) = picture_server(bytes.clone()).await;
    let path = std::env::temp_dir().join(format!("a2a-lab-ot2-image-{}.jpg", std::process::id()));
    let output = bin()
        .args([
            "--opentrons-url",
            &url,
            "--capture-timeout-ms",
            "2000",
            "get-current-image",
            "--source",
            "opentrons-camera",
            "--output",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "hits={} url={url}\n{stderr}\n{stdout}",
        hits.load(Ordering::SeqCst)
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(stdout.contains("a2a-lab get_current_image"));
    assert!(stdout.contains("source\topentrons-camera"));
    assert!(stdout.contains("media_type\timage/jpeg"));
    assert!(stdout.contains("width\t3"));
    assert!(stdout.contains("height\t2"));
    assert!(stdout.contains(&format!("bytes\t{}", bytes.len())));
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    assert!(!stdout.contains(&encoded));
    assert!(
        !stdout
            .as_bytes()
            .windows(2)
            .any(|pair| pair == [0xFF, 0xD8])
    );
    std::fs::remove_file(&path).ok();
}
