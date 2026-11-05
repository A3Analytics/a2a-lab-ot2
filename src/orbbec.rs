//! Opt-in Orbbec color stills through a dynamically loaded Orbbec SDK.

#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{A2aLabError, ImageSource, ImageSourceId, ImageTransportConfig, JsonObject};

use crate::images::{
    CapturedFrame, ImageCapture, ImageCatalogConfig, LiveImageCatalog, jpeg_dimensions,
};
use crate::time::now;

/// Stable image-source id for basic Orbbec color capture.
pub const ORBBEC_CAMERA_SOURCE_ID: &str = "orbbec-camera";

/// Description advertised for [`ORBBEC_CAMERA_SOURCE_ID`].
pub const DEFAULT_ORBBEC_CAMERA_DESCRIPTION: &str =
    "Still color frame from the configured Orbbec camera.";

/// Pinned Orbbec SDK release expected in the agent image.
pub const ORBBEC_SDK_VERSION: &str = "2.10.6";

/// Budget for the first color frame after the pipeline starts.
///
/// A cold Gemini stream returns that frame after the shared still-capture
/// budget has already elapsed. Later frames from the running camera are faster.
pub const ORBBEC_FIRST_FRAME_WAIT: Duration = Duration::from_secs(20);

const SENSOR_COLOR: i32 = 2;
const FORMAT_MJPG: i32 = 5;

/// Failure from the Orbbec capture boundary before catalog storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrbbecCaptureError {
    /// `libOrbbecSDK` could not be opened.
    Library(String),
    /// Enumeration found no device.
    NoDevice,
    /// The requested serial was not enumerated.
    MissingSerial(String),
    /// More than one device was present and no serial was configured.
    Ambiguous(Vec<String>),
    /// The device node or SDK rejected access.
    Permission(String),
    /// The device has no MJPEG color profile.
    NoColorProfile(String),
    /// The SDK returned an operational error.
    Sdk(String),
    /// No color frame arrived before the deadline.
    Timeout,
    /// The SDK returned no bytes.
    Empty,
    /// The encoded bytes were not a JPEG.
    Malformed(String),
    /// The encoded frame exceeded the configured maximum.
    Oversize,
}

impl std::fmt::Display for OrbbecCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library(detail) => {
                write!(
                    formatter,
                    "Orbbec SDK library could not be loaded ({detail})"
                )
            }
            Self::NoDevice => write!(formatter, "no Orbbec device is connected"),
            Self::MissingSerial(serial) => write!(formatter, "serial {serial} was not found"),
            Self::Ambiguous(serials) => write!(
                formatter,
                "multiple Orbbec devices are connected ({}); set --orbbec-serial or A2ALAB_ORBBEC_SERIAL",
                serials.join(", ")
            ),
            Self::Permission(detail) => write!(formatter, "permission denied ({detail})"),
            Self::NoColorProfile(detail) => {
                write!(formatter, "no usable MJPEG color profile ({detail})")
            }
            Self::Sdk(detail) => write!(formatter, "Orbbec SDK failed ({detail})"),
            Self::Timeout => write!(formatter, "timed out"),
            Self::Empty => write!(formatter, "returned an empty frame"),
            Self::Malformed(detail) => write!(formatter, "returned a malformed JPEG ({detail})"),
            Self::Oversize => {
                write!(
                    formatter,
                    "returned a frame larger than the configured maximum"
                )
            }
        }
    }
}

/// Chooses one enumerated device.
///
/// An empty list is no device. A serial must match exactly. Without a serial,
/// the only connected device is used.
pub fn select_device(
    serials: &[String],
    wanted: Option<&str>,
) -> Result<usize, OrbbecCaptureError> {
    if serials.is_empty() {
        return Err(OrbbecCaptureError::NoDevice);
    }
    if let Some(serial) = wanted {
        return serials
            .iter()
            .position(|item| item == serial)
            .ok_or_else(|| OrbbecCaptureError::MissingSerial(serial.to_owned()));
    }
    if serials.len() == 1 {
        return Ok(0);
    }
    let mut listed = serials.to_vec();
    listed.sort();
    Err(OrbbecCaptureError::Ambiguous(listed))
}

/// Captures one MJPEG color frame from a selected Orbbec device.
pub trait OrbbecCapture: Send + Sync {
    /// Returns encoded color bytes for `serial`, or the sole device when `serial` is `None`.
    fn capture_color(
        &self,
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError>;
}

impl OrbbecCapture for crate::cameras::SharedSession {
    fn capture_color(
        &self,
        _serial: Option<&str>,
        timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        self.color_jpeg(timeout).map_err(|error| match error {
            crate::frame_sets::FrameSetError::Timeout { .. } => OrbbecCaptureError::Timeout,
            crate::frame_sets::FrameSetError::Oversize => OrbbecCaptureError::Oversize,
            crate::frame_sets::FrameSetError::Malformed(detail) => {
                OrbbecCaptureError::Malformed(detail)
            }
            other => OrbbecCaptureError::Sdk(other.to_string()),
        })
    }
}

/// Open Orbbec color pipeline that stays running for live video.
pub(crate) struct NativeOrbbecJpeg {
    session: sdk::Session,
    max_bytes: u64,
    pending: Option<Vec<u8>>,
}

impl NativeOrbbecJpeg {
    pub(crate) fn open(serial: Option<&str>, max_bytes: u64) -> Result<Self, A2aLabError> {
        let mut session = sdk::open_color(serial, ORBBEC_FIRST_FRAME_WAIT, max_bytes)
            .map_err(|error| A2aLabError::unavailable(format!("orbbec-camera: {error}")))?;
        let bytes = session
            .next_color(ORBBEC_FIRST_FRAME_WAIT, max_bytes)
            .map_err(|error| A2aLabError::unavailable(format!("orbbec-camera: {error}")))?;
        remember_color_frame(bytes.clone());
        ORBBEC_LIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(Self {
            session,
            max_bytes,
            pending: Some(bytes),
        })
    }

    pub(crate) fn next_frame(&mut self) -> Result<Vec<u8>, A2aLabError> {
        if let Some(bytes) = self.pending.take() {
            return Ok(bytes);
        }
        let bytes = self
            .session
            .next_color(Duration::from_secs(2), self.max_bytes)
            .map_err(|error| A2aLabError::unavailable(format!("orbbec-camera: {error}")))?;
        remember_color_frame(bytes.clone());
        Ok(bytes)
    }
}

impl Drop for NativeOrbbecJpeg {
    fn drop(&mut self) {
        ORBBEC_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Loads `libOrbbecSDK` at capture time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SdkOrbbecCapture;

impl OrbbecCapture for SdkOrbbecCapture {
    fn capture_color(
        &self,
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        let wait = if ORBBEC_LIVE.load(std::sync::atomic::Ordering::Relaxed) {
            timeout.max(ORBBEC_FIRST_FRAME_WAIT)
        } else {
            timeout
        };
        let deadline = std::time::Instant::now() + wait;
        loop {
            if let Some(bytes) = take_remembered_color() {
                return Ok(bytes);
            }
            if !ORBBEC_LIVE.load(std::sync::atomic::Ordering::Relaxed)
                || std::time::Instant::now() >= deadline
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if ORBBEC_LIVE.load(std::sync::atomic::Ordering::Relaxed) {
            return take_remembered_color().ok_or(OrbbecCaptureError::Timeout);
        }
        sdk::capture_color(serial, timeout, max_bytes)
    }
}

/// Keeps the latest synchronized color JPEG for the next `orbbec-camera` read.
pub(crate) fn remember_color_frame(bytes: Vec<u8>) {
    if let Ok(mut slot) = COLOR_REUSE.lock() {
        *slot = Some(bytes);
    }
}

static COLOR_REUSE: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);
static ORBBEC_LIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn take_remembered_color() -> Option<Vec<u8>> {
    COLOR_REUSE.lock().ok().and_then(|slot| slot.clone())
}

/// Reads the loaded Orbbec SDK `major.minor.patch` version.
pub fn loaded_sdk_version() -> Result<String, OrbbecCaptureError> {
    sdk::loaded_version()
}

/// Source descriptor for basic Orbbec color capture.
pub fn orbbec_camera_source() -> Result<ImageSource, A2aLabError> {
    Ok(ImageSource {
        id: ImageSourceId::new(ORBBEC_CAMERA_SOURCE_ID)?,
        name: "Orbbec camera".to_owned(),
        description: DEFAULT_ORBBEC_CAMERA_DESCRIPTION.to_owned(),
        asset_id: None,
        semantic_id: None,
    })
}

/// Live color capture for one logical `orbbec-camera` source.
pub struct OrbbecCamera {
    serial: Option<String>,
    timeout: Duration,
    transport: ImageTransportConfig,
    capture: Arc<dyn OrbbecCapture>,
}

impl OrbbecCamera {
    /// Captures through the Orbbec SDK shared library.
    pub fn new(
        serial: Option<&str>,
        timeout: Duration,
        max_image_bytes: u64,
    ) -> Result<Self, A2aLabError> {
        Self::with_capture(
            serial,
            timeout.max(ORBBEC_FIRST_FRAME_WAIT),
            max_image_bytes,
            SdkOrbbecCapture,
        )
    }

    /// Captures through an injected backend.
    pub fn with_capture(
        serial: Option<&str>,
        timeout: Duration,
        max_image_bytes: u64,
        capture: impl OrbbecCapture + 'static,
    ) -> Result<Self, A2aLabError> {
        Self::with_shared_capture(serial, timeout, max_image_bytes, Arc::new(capture))
    }

    /// Captures through a shared injected backend.
    pub fn with_shared_capture(
        serial: Option<&str>,
        timeout: Duration,
        max_image_bytes: u64,
        capture: Arc<dyn OrbbecCapture>,
    ) -> Result<Self, A2aLabError> {
        if timeout.is_zero() {
            return Err(A2aLabError::invalid("timeout", "must be greater than zero"));
        }
        if serial.is_some_and(str::is_empty) {
            return Err(A2aLabError::invalid("orbbec_serial", "must not be empty"));
        }
        Ok(Self {
            serial: serial.map(str::to_owned),
            timeout,
            transport: ImageTransportConfig::new(max_image_bytes)?,
            capture,
        })
    }

    /// Configured device serial, when selection is pinned.
    #[must_use]
    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    /// Registers this camera as the only source in a process-local catalog.
    pub fn catalog(self, retention_per_source: u64) -> Result<LiveImageCatalog, A2aLabError> {
        let max_image_bytes = self.transport.max_image_bytes();
        LiveImageCatalog::new(
            vec![(orbbec_camera_source()?, Arc::new(self))],
            ImageCatalogConfig::new(retention_per_source, max_image_bytes)?,
        )
    }

    async fn take_frame(&self) -> Result<CapturedFrame, A2aLabError> {
        let body = self.read_frame().await?;
        if body.is_empty() {
            return Err(self.invalid("returned an empty frame"));
        }
        let (width, height) = jpeg_dimensions(&body)
            .map_err(|error| self.invalid(&format!("returned a malformed JPEG ({error})")))?;
        self.transport
            .check_payload(&body)
            .map_err(|_| self.invalid("returned a frame larger than the configured maximum"))?;
        Ok(CapturedFrame::new(
            now()?,
            "image/jpeg",
            width,
            height,
            Some("Orbbec camera".to_owned()),
            JsonObject::empty(),
            body,
        ))
    }

    async fn read_frame(&self) -> Result<Vec<u8>, A2aLabError> {
        let capture = Arc::clone(&self.capture);
        let serial = self.serial.clone();
        let timeout = self.timeout;
        let max_bytes = self.transport.max_image_bytes();
        let task = tokio::task::spawn_blocking(move || {
            crate::cameras::OrbbecColorBackend::new(capture).capture_color_compat(
                serial.as_deref(),
                timeout,
                max_bytes,
            )
        });
        match tokio::time::timeout(self.timeout, task).await {
            Ok(Ok(Ok(bytes))) => Ok(bytes),
            Ok(Ok(Err(error))) => Err(self.capture_error(error)),
            Ok(Err(error)) => Err(self.unavailable(&format!("capture task failed: {error}"))),
            Err(_) => Err(self.unavailable("timed out")),
        }
    }

    fn capture_error(&self, error: OrbbecCaptureError) -> A2aLabError {
        match error {
            OrbbecCaptureError::Empty => self.invalid("returned an empty frame"),
            OrbbecCaptureError::Malformed(detail) => {
                self.invalid(&format!("returned a malformed JPEG ({detail})"))
            }
            OrbbecCaptureError::Oversize => {
                self.invalid("returned a frame larger than the configured maximum")
            }
            other => self.unavailable(&other.to_string()),
        }
    }

    fn label(&self) -> String {
        match &self.serial {
            Some(serial) => format!("orbbec-camera serial {serial}"),
            None => "orbbec-camera".to_owned(),
        }
    }

    fn unavailable(&self, reason: &str) -> A2aLabError {
        A2aLabError::unavailable(format!("{}: {reason}", self.label()))
    }

    fn invalid(&self, reason: &str) -> A2aLabError {
        A2aLabError::invalid("camera", format!("{}: {reason}", self.label()))
    }
}

impl ImageCapture for OrbbecCamera {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async move { self.take_frame().await })
    }
}

mod sdk {
    use super::{FORMAT_MJPG, ORBBEC_SDK_VERSION, OrbbecCaptureError, SENSOR_COLOR, select_device};
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::ptr;
    use std::time::Duration;

    type ErrorSlot = *mut *mut c_void;
    type GetInt = unsafe extern "C" fn() -> i32;
    type CreateContext = unsafe extern "C" fn(ErrorSlot) -> *mut c_void;
    type DeleteContext = unsafe extern "C" fn(*mut c_void, ErrorSlot);
    type EnableNet = unsafe extern "C" fn(*mut c_void, u8, ErrorSlot);
    type QueryList = unsafe extern "C" fn(*mut c_void, ErrorSlot) -> *mut c_void;
    type ListCount = unsafe extern "C" fn(*const c_void, ErrorSlot) -> u32;
    type ListSerial = unsafe extern "C" fn(*const c_void, u32, ErrorSlot) -> *const c_char;
    type ListDevice = unsafe extern "C" fn(*const c_void, u32, ErrorSlot) -> *mut c_void;
    type DeleteList = unsafe extern "C" fn(*mut c_void, ErrorSlot);
    type DeleteDevice = unsafe extern "C" fn(*mut c_void, ErrorSlot);
    type CreatePipeline = unsafe extern "C" fn(*const c_void, ErrorSlot) -> *mut c_void;
    type DeletePipeline = unsafe extern "C" fn(*mut c_void, ErrorSlot);
    type ProfileList = unsafe extern "C" fn(*const c_void, i32, ErrorSlot) -> *mut c_void;
    type MatchVideo =
        unsafe extern "C" fn(*const c_void, i32, i32, i32, i32, ErrorSlot) -> *mut c_void;
    type DeleteProfile = unsafe extern "C" fn(*const c_void, ErrorSlot);
    type DeleteProfileList = unsafe extern "C" fn(*const c_void, ErrorSlot);
    type CreateConfig = unsafe extern "C" fn(ErrorSlot) -> *mut c_void;
    type DeleteConfig = unsafe extern "C" fn(*mut c_void, ErrorSlot);
    type EnableProfile = unsafe extern "C" fn(*mut c_void, *const c_void, ErrorSlot);
    type StartConfig = unsafe extern "C" fn(*mut c_void, *const c_void, ErrorSlot);
    type StopPipeline = unsafe extern "C" fn(*mut c_void, ErrorSlot);
    type WaitFrames = unsafe extern "C" fn(*mut c_void, u32, ErrorSlot) -> *mut c_void;
    type ColorFrame = unsafe extern "C" fn(*const c_void, ErrorSlot) -> *mut c_void;
    type FrameData = unsafe extern "C" fn(*const c_void, ErrorSlot) -> *mut u8;
    type FrameSize = unsafe extern "C" fn(*const c_void, ErrorSlot) -> u32;
    type FrameFormat = unsafe extern "C" fn(*const c_void, ErrorSlot) -> i32;
    type DeleteFrame = unsafe extern "C" fn(*const c_void, ErrorSlot);
    type ErrorMessage = unsafe extern "C" fn(*const c_void) -> *const c_char;
    type DeleteError = unsafe extern "C" fn(*mut c_void);
    type SetExtensions = unsafe extern "C" fn(*const c_char, ErrorSlot);

    struct Api {
        _library: libloading::Library,
        get_major: GetInt,
        get_minor: GetInt,
        get_patch: GetInt,
        create_context: CreateContext,
        delete_context: DeleteContext,
        enable_net: EnableNet,
        query_list: QueryList,
        list_count: ListCount,
        list_serial: ListSerial,
        list_device: ListDevice,
        delete_list: DeleteList,
        delete_device: DeleteDevice,
        create_pipeline: CreatePipeline,
        delete_pipeline: DeletePipeline,
        profile_list: ProfileList,
        match_video: MatchVideo,
        delete_profile: DeleteProfile,
        delete_profile_list: DeleteProfileList,
        create_config: CreateConfig,
        delete_config: DeleteConfig,
        enable_profile: EnableProfile,
        start_config: StartConfig,
        stop_pipeline: StopPipeline,
        wait_frames: WaitFrames,
        color_frame: ColorFrame,
        frame_data: FrameData,
        frame_size: FrameSize,
        frame_format: FrameFormat,
        delete_frame: DeleteFrame,
        error_message: ErrorMessage,
        delete_error: DeleteError,
        set_extensions: SetExtensions,
    }

    pub(super) struct Session {
        api: Api,
        context: *mut c_void,
        list: *mut c_void,
        device: *mut c_void,
        pipeline: *mut c_void,
        profiles: *mut c_void,
        profile: *mut c_void,
        config: *mut c_void,
        frameset: *mut c_void,
        frame: *mut c_void,
        started: bool,
    }

    pub(super) fn loaded_version() -> Result<String, OrbbecCaptureError> {
        let api = Api::load()?;
        let (major, minor, patch) =
            unsafe { ((api.get_major)(), (api.get_minor)(), (api.get_patch)()) };
        let version = format!("{major}.{minor}.{patch}");
        if version != ORBBEC_SDK_VERSION {
            return Err(OrbbecCaptureError::Sdk(format!(
                "loaded Orbbec SDK {version}, expected {ORBBEC_SDK_VERSION}"
            )));
        }
        Ok(version)
    }

    pub(super) fn capture_color(
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        let mut session = open_color(serial, timeout, max_bytes)?;
        session.next_color(timeout, max_bytes)
    }

    pub(super) fn open_color(
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Session, OrbbecCaptureError> {
        let api = Api::load()?;
        let mut session = Session::new(api);
        session.start(serial, timeout, max_bytes)?;
        Ok(session)
    }

    impl Api {
        fn load() -> Result<Self, OrbbecCaptureError> {
            let mut names = Vec::new();
            if let Ok(path) = std::env::var("A2ALAB_ORBBEC_SDK_LIBRARY")
                && !path.is_empty()
            {
                names.push(path);
            }
            names.extend(["libOrbbecSDK.so.2".to_owned(), "libOrbbecSDK.so".to_owned()]);
            let mut failures = Vec::new();
            for name in names {
                match unsafe { libloading::Library::new(&name) } {
                    Ok(library) => return Self::bind(library),
                    Err(error) => failures.push(format!("{name}: {error}")),
                }
            }
            Err(OrbbecCaptureError::Library(failures.join("; ")))
        }

        fn bind(library: libloading::Library) -> Result<Self, OrbbecCaptureError> {
            unsafe {
                Ok(Self {
                    get_major: symbol(&library, b"ob_get_major_version")?,
                    get_minor: symbol(&library, b"ob_get_minor_version")?,
                    get_patch: symbol(&library, b"ob_get_patch_version")?,
                    create_context: symbol(&library, b"ob_create_context")?,
                    delete_context: symbol(&library, b"ob_delete_context")?,
                    enable_net: symbol(&library, b"ob_enable_net_device_enumeration")?,
                    query_list: symbol(&library, b"ob_query_device_list")?,
                    list_count: symbol(&library, b"ob_device_list_get_count")?,
                    list_serial: symbol(&library, b"ob_device_list_get_device_serial_number")?,
                    list_device: symbol(&library, b"ob_device_list_get_device")?,
                    delete_list: symbol(&library, b"ob_delete_device_list")?,
                    delete_device: symbol(&library, b"ob_delete_device")?,
                    create_pipeline: symbol(&library, b"ob_create_pipeline_with_device")?,
                    delete_pipeline: symbol(&library, b"ob_delete_pipeline")?,
                    profile_list: symbol(&library, b"ob_pipeline_get_stream_profile_list")?,
                    match_video: symbol(
                        &library,
                        b"ob_stream_profile_list_get_video_stream_profile",
                    )?,
                    delete_profile: symbol(&library, b"ob_delete_stream_profile")?,
                    delete_profile_list: symbol(&library, b"ob_delete_stream_profile_list")?,
                    create_config: symbol(&library, b"ob_create_config")?,
                    delete_config: symbol(&library, b"ob_delete_config")?,
                    enable_profile: symbol(
                        &library,
                        b"ob_config_enable_stream_with_stream_profile",
                    )?,
                    start_config: symbol(&library, b"ob_pipeline_start_with_config")?,
                    stop_pipeline: symbol(&library, b"ob_pipeline_stop")?,
                    wait_frames: symbol(&library, b"ob_pipeline_wait_for_frameset")?,
                    color_frame: symbol(&library, b"ob_frameset_get_color_frame")?,
                    frame_data: symbol(&library, b"ob_frame_get_data")?,
                    frame_size: symbol(&library, b"ob_frame_get_data_size")?,
                    frame_format: symbol(&library, b"ob_frame_get_format")?,
                    delete_frame: symbol(&library, b"ob_delete_frame")?,
                    error_message: symbol(&library, b"ob_error_get_message")?,
                    delete_error: symbol(&library, b"ob_delete_error")?,
                    set_extensions: symbol(&library, b"ob_set_extensions_directory")?,
                    _library: library,
                })
            }
        }

        fn check(&self, error: *mut c_void) -> Result<(), OrbbecCaptureError> {
            let Some(message) = self.consume_error(error) else {
                return Ok(());
            };
            if permission(&message) {
                Err(OrbbecCaptureError::Permission(message))
            } else {
                Err(OrbbecCaptureError::Sdk(message))
            }
        }

        fn consume_error(&self, error: *mut c_void) -> Option<String> {
            if error.is_null() {
                return None;
            }
            let message = unsafe {
                let ptr = (self.error_message)(error);
                if ptr.is_null() {
                    "unknown Orbbec SDK error".to_owned()
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            unsafe { (self.delete_error)(error) };
            Some(message)
        }
    }

    impl Session {
        fn new(api: Api) -> Self {
            Self {
                api,
                context: ptr::null_mut(),
                list: ptr::null_mut(),
                device: ptr::null_mut(),
                pipeline: ptr::null_mut(),
                profiles: ptr::null_mut(),
                profile: ptr::null_mut(),
                config: ptr::null_mut(),
                frameset: ptr::null_mut(),
                frame: ptr::null_mut(),
                started: false,
            }
        }

        fn start(
            &mut self,
            serial: Option<&str>,
            timeout: Duration,
            max_bytes: u64,
        ) -> Result<(), OrbbecCaptureError> {
            let _ = (timeout, max_bytes);
            self.prepare_extensions()?;
            self.open_context()?;
            let index = self.select(serial)?;
            self.open_pipeline(index)?;
            self.enable_color()
        }

        pub(super) fn next_color(
            &mut self,
            timeout: Duration,
            max_bytes: u64,
        ) -> Result<Vec<u8>, OrbbecCaptureError> {
            self.release_frames();
            self.read_color(timeout, max_bytes)
        }

        fn release_frames(&mut self) {
            unsafe {
                self.release(self.frame, self.api.delete_frame);
                self.release(self.frameset, self.api.delete_frame);
            }
            self.frame = ptr::null_mut();
            self.frameset = ptr::null_mut();
        }

        fn prepare_extensions(&self) -> Result<(), OrbbecCaptureError> {
            let Ok(path) = std::env::var("A2ALAB_ORBBEC_EXTENSIONS") else {
                return Ok(());
            };
            if path.is_empty() {
                return Ok(());
            }
            let path = CString::new(path).map_err(|_| {
                OrbbecCaptureError::Sdk("Orbbec extensions path contains a null byte".to_owned())
            })?;
            let mut error = ptr::null_mut();
            unsafe { (self.api.set_extensions)(path.as_ptr(), &raw mut error) };
            self.api.check(error)
        }

        fn open_context(&mut self) -> Result<(), OrbbecCaptureError> {
            let mut error = ptr::null_mut();
            let context = unsafe { (self.api.create_context)(&raw mut error) };
            self.api.check(error)?;
            if context.is_null() {
                return Err(OrbbecCaptureError::Sdk(
                    "context was not created".to_owned(),
                ));
            }
            self.context = context;
            let mut error = ptr::null_mut();
            unsafe { (self.api.enable_net)(context, 0, &raw mut error) };
            self.api.check(error)?;
            let mut error = ptr::null_mut();
            let list = unsafe { (self.api.query_list)(context, &raw mut error) };
            self.api.check(error)?;
            if list.is_null() {
                return Err(OrbbecCaptureError::NoDevice);
            }
            self.list = list;
            Ok(())
        }

        fn select(&self, serial: Option<&str>) -> Result<u32, OrbbecCaptureError> {
            let mut error = ptr::null_mut();
            let count = unsafe { (self.api.list_count)(self.list, &raw mut error) };
            self.api.check(error)?;
            let mut serials = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
            for index in 0..count {
                serials.push(self.serial_at(index)?);
            }
            let chosen = select_device(&serials, serial)?;
            u32::try_from(chosen)
                .map_err(|_| OrbbecCaptureError::Sdk("device index overflow".to_owned()))
        }

        fn serial_at(&self, index: u32) -> Result<String, OrbbecCaptureError> {
            let mut error = ptr::null_mut();
            let ptr = unsafe { (self.api.list_serial)(self.list, index, &raw mut error) };
            self.api.check(error)?;
            if ptr.is_null() {
                return Ok(String::new());
            }
            Ok(unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned())
        }

        fn open_pipeline(&mut self, index: u32) -> Result<(), OrbbecCaptureError> {
            let mut error = ptr::null_mut();
            let device = unsafe { (self.api.list_device)(self.list, index, &raw mut error) };
            self.api.check(error)?;
            if device.is_null() {
                return Err(OrbbecCaptureError::Sdk("device was not opened".to_owned()));
            }
            self.device = device;
            let mut error = ptr::null_mut();
            let pipeline = unsafe { (self.api.create_pipeline)(device, &raw mut error) };
            self.api.check(error)?;
            if pipeline.is_null() {
                return Err(OrbbecCaptureError::Sdk(
                    "pipeline was not created".to_owned(),
                ));
            }
            self.pipeline = pipeline;
            Ok(())
        }

        fn enable_color(&mut self) -> Result<(), OrbbecCaptureError> {
            let mut error = ptr::null_mut();
            let profiles =
                unsafe { (self.api.profile_list)(self.pipeline, SENSOR_COLOR, &raw mut error) };
            if let Err(err) = self.api.check(error) {
                return Err(profile_error(err));
            }
            if profiles.is_null() {
                return Err(OrbbecCaptureError::NoColorProfile(
                    "color sensor profile list was empty".to_owned(),
                ));
            }
            self.profiles = profiles;
            let mut error = ptr::null_mut();
            let profile =
                unsafe { (self.api.match_video)(profiles, 0, 0, FORMAT_MJPG, 0, &raw mut error) };
            if let Err(err) = self.api.check(error) {
                return Err(profile_error(err));
            }
            if profile.is_null() {
                return Err(OrbbecCaptureError::NoColorProfile(
                    "no MJPEG color profile".to_owned(),
                ));
            }
            self.profile = profile;
            let mut error = ptr::null_mut();
            let config = unsafe { (self.api.create_config)(&raw mut error) };
            self.api.check(error)?;
            if config.is_null() {
                return Err(OrbbecCaptureError::Sdk(
                    "pipeline config was not created".to_owned(),
                ));
            }
            self.config = config;
            let mut error = ptr::null_mut();
            unsafe { (self.api.enable_profile)(config, profile, &raw mut error) };
            if let Err(err) = self.api.check(error) {
                return Err(profile_error(err));
            }
            let mut error = ptr::null_mut();
            unsafe { (self.api.start_config)(self.pipeline, config, &raw mut error) };
            self.api.check(error)?;
            self.started = true;
            Ok(())
        }

        fn read_color(
            &mut self,
            timeout: Duration,
            max_bytes: u64,
        ) -> Result<Vec<u8>, OrbbecCaptureError> {
            let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
            let mut error = ptr::null_mut();
            let frameset = unsafe { (self.api.wait_frames)(self.pipeline, millis, &raw mut error) };
            self.api.check(error)?;
            if frameset.is_null() {
                return Err(OrbbecCaptureError::Timeout);
            }
            self.frameset = frameset;
            let mut error = ptr::null_mut();
            let frame = unsafe { (self.api.color_frame)(frameset, &raw mut error) };
            self.api.check(error)?;
            if frame.is_null() {
                return Err(OrbbecCaptureError::Timeout);
            }
            self.frame = frame;
            let mut error = ptr::null_mut();
            let format = unsafe { (self.api.frame_format)(frame, &raw mut error) };
            self.api.check(error)?;
            if format != FORMAT_MJPG {
                return Err(OrbbecCaptureError::Malformed(format!(
                    "color frame format {format} was not MJPEG"
                )));
            }
            let mut error = ptr::null_mut();
            let size = unsafe { (self.api.frame_size)(frame, &raw mut error) };
            self.api.check(error)?;
            if size == 0 {
                return Err(OrbbecCaptureError::Empty);
            }
            let len = usize::try_from(size).map_err(|_| OrbbecCaptureError::Oversize)?;
            let cap = usize::try_from(max_bytes).map_err(|_| OrbbecCaptureError::Oversize)?;
            if len > cap {
                return Err(OrbbecCaptureError::Oversize);
            }
            let mut error = ptr::null_mut();
            let data = unsafe { (self.api.frame_data)(frame, &raw mut error) };
            self.api.check(error)?;
            if data.is_null() {
                return Err(OrbbecCaptureError::Empty);
            }
            Ok(unsafe { std::slice::from_raw_parts(data, len) }.to_vec())
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                self.release(self.frame, self.api.delete_frame);
                self.release(self.frameset, self.api.delete_frame);
                if self.started && !self.pipeline.is_null() {
                    let mut error = ptr::null_mut();
                    (self.api.stop_pipeline)(self.pipeline, &raw mut error);
                    let _ = self.api.consume_error(error);
                }
                self.release(self.profile, self.api.delete_profile);
                self.release(self.profiles, self.api.delete_profile_list);
                if !self.config.is_null() {
                    let mut error = ptr::null_mut();
                    (self.api.delete_config)(self.config, &raw mut error);
                    let _ = self.api.consume_error(error);
                }
                if !self.pipeline.is_null() {
                    let mut error = ptr::null_mut();
                    (self.api.delete_pipeline)(self.pipeline, &raw mut error);
                    let _ = self.api.consume_error(error);
                }
                if !self.device.is_null() {
                    let mut error = ptr::null_mut();
                    (self.api.delete_device)(self.device, &raw mut error);
                    let _ = self.api.consume_error(error);
                }
                if !self.list.is_null() {
                    let mut error = ptr::null_mut();
                    (self.api.delete_list)(self.list, &raw mut error);
                    let _ = self.api.consume_error(error);
                }
                if !self.context.is_null() {
                    let mut error = ptr::null_mut();
                    (self.api.delete_context)(self.context, &raw mut error);
                    let _ = self.api.consume_error(error);
                }
            }
        }
    }

    impl Session {
        unsafe fn release(
            &self,
            value: *mut c_void,
            delete: unsafe extern "C" fn(*const c_void, *mut *mut c_void),
        ) {
            if value.is_null() {
                return;
            }
            let mut error = ptr::null_mut();
            unsafe {
                delete(value, &raw mut error);
            }
            let _ = self.api.consume_error(error);
        }
    }

    unsafe fn symbol<T: Copy>(
        library: &libloading::Library,
        name: &[u8],
    ) -> Result<T, OrbbecCaptureError> {
        let symbol = unsafe { library.get::<T>(name) }.map_err(|error| {
            OrbbecCaptureError::Library(format!("{}: {error}", String::from_utf8_lossy(name)))
        })?;
        Ok(*symbol)
    }

    fn permission(message: &str) -> bool {
        let lower = message.to_ascii_lowercase();
        lower.contains("permission")
            || lower.contains("access denied")
            || lower.contains("operation not permitted")
            || lower.contains("libusb_error_access")
            || lower.contains("errno=13")
    }

    fn profile_error(error: OrbbecCaptureError) -> OrbbecCaptureError {
        match error {
            OrbbecCaptureError::Sdk(detail) => OrbbecCaptureError::NoColorProfile(detail),
            other => other,
        }
    }

    #[cfg(test)]
    mod version_shape {
        use super::ORBBEC_SDK_VERSION;

        #[test]
        fn pinned_version_has_three_numeric_parts() {
            let mut parts = ORBBEC_SDK_VERSION.split('.');
            assert!(parts.next().unwrap().parse::<u32>().is_ok());
            assert!(parts.next().unwrap().parse::<u32>().is_ok());
            assert!(parts.next().unwrap().parse::<u32>().is_ok());
            assert!(parts.next().is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        OrbbecCapture, OrbbecCaptureError, SdkOrbbecCapture, remember_color_frame, select_device,
    };

    #[test]
    fn device_selection_covers_none_one_serial_and_many() {
        assert_eq!(select_device(&[], None), Err(OrbbecCaptureError::NoDevice));
        assert_eq!(select_device(&["A".to_owned()], None).unwrap(), 0);
        assert_eq!(
            select_device(&["A".to_owned(), "B".to_owned()], Some("B")).unwrap(),
            1
        );
        assert_eq!(
            select_device(&["A".to_owned()], Some("missing")),
            Err(OrbbecCaptureError::MissingSerial("missing".to_owned()))
        );
        assert_eq!(
            select_device(&["B".to_owned(), "A".to_owned()], None),
            Err(OrbbecCaptureError::Ambiguous(vec![
                "A".to_owned(),
                "B".to_owned()
            ]))
        );
    }

    #[test]
    fn remembered_color_is_returned_without_loading_the_sdk() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
        remember_color_frame(jpeg.clone());
        let bytes = SdkOrbbecCapture
            .capture_color(None, Duration::from_millis(1), 64)
            .unwrap();
        assert_eq!(bytes, jpeg);
    }
}
