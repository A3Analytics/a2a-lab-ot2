//! External vs Orbbec media contract parity across provider, A2A, MCP, and gateway.

#[path = "support/media_parity_matrix.rs"]
mod matrix;

pub(crate) use matrix::INTENTIONAL_MEDIA_PARITY_DIFFERENCES;

#[test]
fn intentional_media_parity_differences_are_recorded() {
    for phrase in [
        "external-camera",
        "orbbec-camera",
        "native camera index",
        "OrbbecSDK",
        "serial",
        "twenty-second",
        "20-second",
        "ORBBEC_FIRST_FRAME_WAIT",
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
async fn external_orbbec_media_parity_matrix() {
    matrix::run().await;
}
