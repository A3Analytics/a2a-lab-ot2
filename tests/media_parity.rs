//! External vs webcam media contract parity across provider, A2A, MCP, and gateway.

#[path = "support/media_parity_matrix.rs"]
mod matrix;

pub(crate) use matrix::INTENTIONAL_MEDIA_PARITY_DIFFERENCES;

#[test]
fn intentional_media_parity_differences_are_recorded() {
    for phrase in [
        "external-camera",
        "camera",
        "native camera index",
        "--camera-index",
        "does not open a second device",
        "diagnostic",
        "Nokhwa",
    ] {
        assert!(
            INTENTIONAL_MEDIA_PARITY_DIFFERENCES.contains(phrase),
            "missing documented difference: {phrase}"
        );
    }
}

#[tokio::test]
async fn external_camera_media_parity_matrix() {
    matrix::run().await;
}
