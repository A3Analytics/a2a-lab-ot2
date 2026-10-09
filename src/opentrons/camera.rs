//! OT-2 on-board camera still capture.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{A2aLabError, ImageSource, ImageSourceId, ImageTransportConfig, JsonObject};

use crate::images::{
    CapturedFrame, ImageCapture, ImageCatalogConfig, LiveImageCatalog, jpeg_dimensions,
};
use crate::time::now;

use super::client::OpentronsClient;

/// Stable image-source id for the OT-2 on-board camera.
pub const OPENTRONS_CAMERA_SOURCE_ID: &str = "opentrons-camera";

/// Industrial asset id associated with the OT-2 camera source.
pub const OPENTRONS_CAMERA_ASSET_ID: &str = "opentrons-ot2";

/// Source descriptor for the on-board camera.
///
/// Capture uses pinned robot-server `POST /camera/picture`. The agent does not
/// enable the camera, change its settings, or read `GET /camera/stream`.
pub fn opentrons_camera_source() -> Result<ImageSource, A2aLabError> {
    Ok(ImageSource {
        id: ImageSourceId::new(OPENTRONS_CAMERA_SOURCE_ID)?,
        name: "Opentrons camera".to_owned(),
        description: "Still frame from the OT-2 on-board camera via POST /camera/picture."
            .to_owned(),
        asset_id: Some(OPENTRONS_CAMERA_ASSET_ID.to_owned()),
        semantic_id: None,
    })
}

/// Live capture of the OT-2 on-board camera through robot-server.
#[derive(Clone)]
pub struct OpentronsCamera {
    client: OpentronsClient,
    timeout: Duration,
    transport: ImageTransportConfig,
}

impl OpentronsCamera {
    /// Builds a capture client for `base_url`.
    ///
    /// `timeout` bounds `POST /camera/picture`. `max_image_bytes` is the decoded
    /// payload ceiling. Neither value may be zero.
    pub fn new(
        base_url: &str,
        timeout: Duration,
        max_image_bytes: u64,
    ) -> Result<Self, A2aLabError> {
        if timeout.is_zero() {
            return Err(A2aLabError::invalid("timeout", "must be greater than zero"));
        }
        Ok(Self {
            client: OpentronsClient::new(base_url)?,
            timeout,
            transport: ImageTransportConfig::new(max_image_bytes)?,
        })
    }

    /// Registers this camera as the only source in a process-local catalog.
    pub fn catalog(self, retention_per_source: u64) -> Result<LiveImageCatalog, A2aLabError> {
        let max_image_bytes = self.transport.max_image_bytes();
        LiveImageCatalog::new(
            vec![(opentrons_camera_source()?, Arc::new(self))],
            ImageCatalogConfig::new(retention_per_source, max_image_bytes)?,
        )
    }

    async fn take_frame(&self) -> Result<CapturedFrame, A2aLabError> {
        let (content_type, body) = self.client.camera_picture(self.timeout).await?;
        let media_type = jpeg_content_type(&content_type)?;
        if body.is_empty() {
            return Err(A2aLabError::invalid(
                "camera",
                "OT-2 camera POST /camera/picture returned an empty body",
            ));
        }
        let (width, height) = jpeg_dimensions(&body).map_err(|error| {
            A2aLabError::invalid(
                "camera",
                format!("OT-2 camera POST /camera/picture returned a malformed JPEG: {error}"),
            )
        })?;
        self.transport.check_payload(&body).map_err(|error| {
            A2aLabError::invalid("camera", format!("OT-2 camera frame rejected: {error}"))
        })?;
        Ok(CapturedFrame::new(
            now()?,
            media_type,
            width,
            height,
            Some("OT-2 on-board camera".to_owned()),
            JsonObject::empty(),
            body,
        ))
    }
}

impl ImageCapture for OpentronsCamera {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async move { self.take_frame().await })
    }
}

fn jpeg_content_type(header: &str) -> Result<&'static str, A2aLabError> {
    let mime = header.split(';').next().unwrap_or("").trim();
    if mime.eq_ignore_ascii_case("image/jpeg") || mime.eq_ignore_ascii_case("image/jpg") {
        Ok("image/jpeg")
    } else {
        Err(A2aLabError::invalid(
            "camera",
            format!("OT-2 camera POST /camera/picture returned {header:?}, expected image/jpeg"),
        ))
    }
}
