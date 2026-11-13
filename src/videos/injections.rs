//! Hardware-free camera overrides for declarative runtime tests and legacy serve slots.

use std::collections::HashMap;

use a2a_lab_dev_kit::VideoRuntimeInjections;
use a2a_lab_dev_kit::videos::ColorCamera;

use crate::camera::{CAMERA_SOURCE_ID, EXTERNAL_CAMERA_SOURCE_ID};
use crate::videos::external::LiveJpegSource;
use crate::videos::jpeg_camera::JpegSourceCamera;

/// Injectable producers for hardware-free tests.
#[derive(Default)]
pub struct LiveVideoInjections {
    /// Injected MJPEG pumps keyed by configured source id.
    pub jpeg_sources: HashMap<String, Box<dyn LiveJpegSource>>,
    /// Injected [`ColorCamera`] backends keyed by stable source id.
    pub cameras: HashMap<String, Box<dyn ColorCamera + Send>>,
}

impl LiveVideoInjections {
    /// No injected producers.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Schema-default JPEG pump overrides for legacy serve configuration only.
    #[must_use]
    pub fn legacy_jpeg(
        external: Option<Box<dyn LiveJpegSource>>,
        webcam: Option<Box<dyn LiveJpegSource>>,
    ) -> Self {
        let mut jpeg_sources = HashMap::new();
        if let Some(source) = external {
            jpeg_sources.insert(EXTERNAL_CAMERA_SOURCE_ID.to_owned(), source);
        }
        if let Some(source) = webcam {
            jpeg_sources.insert(CAMERA_SOURCE_ID.to_owned(), source);
        }
        Self {
            jpeg_sources,
            cameras: HashMap::new(),
        }
    }

    /// Registers an injected MJPEG producer for `id`.
    #[must_use]
    pub fn with_jpeg_source(
        mut self,
        id: impl Into<String>,
        source: Box<dyn LiveJpegSource>,
    ) -> Self {
        self.jpeg_sources.insert(id.into(), source);
        self
    }

    /// Registers an injected camera for `id`.
    #[must_use]
    pub fn with_color_camera(
        mut self,
        id: impl Into<String>,
        camera: Box<dyn ColorCamera + Send>,
    ) -> Self {
        self.cameras.insert(id.into(), camera);
        self
    }

    /// Maps legacy injection slots into devkit [`VideoRuntimeInjections`].
    #[must_use]
    pub fn into_runtime_injections(mut self) -> VideoRuntimeInjections {
        let mut runtime = VideoRuntimeInjections::none();
        for (id, camera) in self.cameras.drain() {
            runtime = runtime.with_camera(id, camera);
        }
        for (id, source) in self.jpeg_sources.drain() {
            runtime = runtime.with_camera(
                id.clone(),
                Box::new(JpegSourceCamera::for_legacy_source_id(&id, source)),
            );
        }
        runtime
    }
}

/// External camera index and description when that source is enabled.
pub struct ExternalVideoConfig<'a> {
    /// Native camera index.
    pub index: u32,
    /// Advertised description.
    pub description: &'a str,
}

/// Second webcam index when source `camera` is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebcamVideoConfig {
    /// V4L2 index opened with the closest 1280×720 MJPEG mode.
    pub index: u32,
}
