//! Synchronized Orbbec framesets loaded through `libOrbbecSDK`.
//!
//! The color-only path keeps its own bindings. A missing frameset symbol fails
//! this path and leaves JPEG capture usable.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::time::Duration;

use crate::orbbec::{ORBBEC_SDK_VERSION, OrbbecCaptureError, select_device};

const SENSOR_IR: i32 = 1;
const SENSOR_COLOR: i32 = 2;
const SENSOR_DEPTH: i32 = 3;
const SENSOR_IR_LEFT: i32 = 6;
const SENSOR_IR_RIGHT: i32 = 7;
const SENSOR_CONFIDENCE: i32 = 9;
const FRAME_IR: i32 = 1;
const FRAME_COLOR: i32 = 2;
const FRAME_DEPTH: i32 = 3;
const FRAME_IR_LEFT: i32 = 8;
const FRAME_IR_RIGHT: i32 = 9;
const FRAME_CONFIDENCE: i32 = 11;
const FORMAT_MJPG: i32 = 5;
const FORMAT_Y16: i32 = 8;
const FORMAT_Y8: i32 = 9;

/// Pixel layout of one SDK plane before PNG or JPEG packaging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneFormat {
    /// MJPEG color.
    Jpeg,
    /// 8-bit grayscale.
    Gray8,
    /// Little-endian 16-bit grayscale.
    Gray16Le,
}

/// Pinhole intrinsics copied out of the SDK struct.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CameraIntrinsics {
    /// Focal length x.
    pub fx: f32,
    /// Focal length y.
    pub fy: f32,
    /// Principal point x.
    pub cx: f32,
    /// Principal point y.
    pub cy: f32,
    /// Calibration width.
    pub width: u32,
    /// Calibration height.
    pub height: u32,
}

/// One plane from a device, with or without sample bytes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plane {
    /// Portable channel id.
    pub channel_id: &'static str,
    /// Portable role.
    pub role: &'static str,
    /// Sample layout.
    pub format: PlaneFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Advertised frame rate.
    pub fps: f64,
    /// Device timestamp in microseconds.
    pub timestamp_us: u64,
    /// Device frame number.
    pub frame_number: u64,
    /// Depth sample multiplier in millimeters.
    pub depth_scale_mm: Option<f32>,
    /// Encoded or raw samples. Empty during profile discovery.
    pub samples: Vec<u8>,
    /// Intrinsics when the profile reported them.
    pub intrinsics: Option<CameraIntrinsics>,
    /// Distortion model and coefficients.
    pub distortion: Option<(String, Vec<f32>)>,
    /// Rotation toward the color camera.
    pub rotation: Option<[f32; 9]>,
    /// Translation toward the color camera, in millimeters.
    pub translation: Option<[f32; 3]>,
}

/// Profiles or frames from one opened device.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeviceView {
    /// Device serial.
    pub serial: String,
    /// Device model name.
    pub model: String,
    /// Channels in capture order.
    pub planes: Vec<Plane>,
}

/// Reads advertised profiles and closes the device without waiting for frames.
pub(crate) fn describe(serial: Option<&str>) -> Result<DeviceView, OrbbecCaptureError> {
    let api = Api::load()?;
    let mut session = Session::new(&api);
    session.open(serial)?;
    session.view(false, Duration::ZERO, 0)
}

/// Waits for one frameset and copies every enabled plane.
pub(crate) fn capture(
    serial: Option<&str>,
    timeout: Duration,
    max_bytes: u64,
) -> Result<DeviceView, OrbbecCaptureError> {
    let api = Api::load()?;
    let mut session = Session::new(&api);
    session.open(serial)?;
    session.view(true, timeout, max_bytes)
}

struct StreamSpec {
    sensor: i32,
    frame_type: i32,
    channel_id: &'static str,
    role: &'static str,
    formats: &'static [i32],
    required: bool,
}

const STREAMS: &[StreamSpec] = &[
    StreamSpec {
        sensor: SENSOR_COLOR,
        frame_type: FRAME_COLOR,
        channel_id: "color",
        role: "color",
        formats: &[FORMAT_MJPG],
        required: true,
    },
    StreamSpec {
        sensor: SENSOR_DEPTH,
        frame_type: FRAME_DEPTH,
        channel_id: "depth",
        role: "depth",
        formats: &[FORMAT_Y16],
        required: false,
    },
    StreamSpec {
        sensor: SENSOR_IR_LEFT,
        frame_type: FRAME_IR_LEFT,
        channel_id: "infrared-left",
        role: "infrared-left",
        formats: &[FORMAT_Y8, FORMAT_Y16],
        required: false,
    },
    StreamSpec {
        sensor: SENSOR_IR_RIGHT,
        frame_type: FRAME_IR_RIGHT,
        channel_id: "infrared-right",
        role: "infrared-right",
        formats: &[FORMAT_Y8, FORMAT_Y16],
        required: false,
    },
    StreamSpec {
        sensor: SENSOR_CONFIDENCE,
        frame_type: FRAME_CONFIDENCE,
        channel_id: "confidence",
        role: "confidence",
        formats: &[FORMAT_Y8, FORMAT_Y16],
        required: false,
    },
];

const INFRARED: StreamSpec = StreamSpec {
    sensor: SENSOR_IR,
    frame_type: FRAME_IR,
    channel_id: "infrared-left",
    role: "infrared-left",
    formats: &[FORMAT_Y8, FORMAT_Y16],
    required: false,
};

#[repr(C)]
struct ObIntrinsic {
    fx: f32,
    fy: f32,
    cx: f32,
    cy: f32,
    width: i16,
    height: i16,
}

#[repr(C)]
struct ObDistortion {
    k1: f32,
    k2: f32,
    k3: f32,
    k4: f32,
    k5: f32,
    k6: f32,
    p1: f32,
    p2: f32,
    model: i32,
}

#[repr(C)]
struct ObExtrinsic {
    rot: [f32; 9],
    trans: [f32; 3],
}

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
type MatchVideo = unsafe extern "C" fn(*const c_void, i32, i32, i32, i32, ErrorSlot) -> *mut c_void;
type DeleteProfile = unsafe extern "C" fn(*const c_void, ErrorSlot);
type DeleteProfileList = unsafe extern "C" fn(*const c_void, ErrorSlot);
type CreateConfig = unsafe extern "C" fn(ErrorSlot) -> *mut c_void;
type DeleteConfig = unsafe extern "C" fn(*mut c_void, ErrorSlot);
type EnableProfile = unsafe extern "C" fn(*mut c_void, *const c_void, ErrorSlot);
type StartConfig = unsafe extern "C" fn(*mut c_void, *const c_void, ErrorSlot);
type StopPipeline = unsafe extern "C" fn(*mut c_void, ErrorSlot);
type WaitFrames = unsafe extern "C" fn(*mut c_void, u32, ErrorSlot) -> *mut c_void;
type FrameByType = unsafe extern "C" fn(*const c_void, i32, ErrorSlot) -> *mut c_void;
type FrameData = unsafe extern "C" fn(*const c_void, ErrorSlot) -> *mut u8;
type FrameSize = unsafe extern "C" fn(*const c_void, ErrorSlot) -> u32;
type FrameFormat = unsafe extern "C" fn(*const c_void, ErrorSlot) -> i32;
type FrameU64 = unsafe extern "C" fn(*const c_void, ErrorSlot) -> u64;
type FrameU32 = unsafe extern "C" fn(*const c_void, ErrorSlot) -> u32;
type FrameF32 = unsafe extern "C" fn(*const c_void, ErrorSlot) -> f32;
type ProfileU32 = unsafe extern "C" fn(*const c_void, ErrorSlot) -> u32;
type GetIntrinsic = unsafe extern "C" fn(*const c_void, ErrorSlot) -> ObIntrinsic;
type GetDistortion = unsafe extern "C" fn(*const c_void, ErrorSlot) -> ObDistortion;
type GetExtrinsic = unsafe extern "C" fn(*const c_void, *const c_void, ErrorSlot) -> ObExtrinsic;
type DeleteFrame = unsafe extern "C" fn(*const c_void, ErrorSlot);
type ErrorMessage = unsafe extern "C" fn(*const c_void) -> *const c_char;
type DeleteError = unsafe extern "C" fn(*mut c_void);
type SetExtensions = unsafe extern "C" fn(*const c_char, ErrorSlot);
type DeviceInfo = unsafe extern "C" fn(*const c_void, ErrorSlot) -> *mut c_void;
type InfoName = unsafe extern "C" fn(*const c_void, ErrorSlot) -> *const c_char;
type DeleteInfo = unsafe extern "C" fn(*mut c_void, ErrorSlot);

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
    frame_by_type: FrameByType,
    frame_data: FrameData,
    frame_size: FrameSize,
    frame_format: FrameFormat,
    frame_index: FrameU64,
    frame_timestamp: FrameU64,
    frame_width: FrameU32,
    frame_height: FrameU32,
    depth_scale: FrameF32,
    profile_width: ProfileU32,
    profile_height: ProfileU32,
    profile_fps: ProfileU32,
    intrinsic: GetIntrinsic,
    distortion: GetDistortion,
    extrinsic: GetExtrinsic,
    delete_frame: DeleteFrame,
    error_message: ErrorMessage,
    delete_error: DeleteError,
    set_extensions: SetExtensions,
    device_info: Option<DeviceInfo>,
    info_name: Option<InfoName>,
    delete_info: Option<DeleteInfo>,
}

struct Matched {
    frame_type: i32,
    channel_id: &'static str,
    role: &'static str,
    format: i32,
    profile: *mut c_void,
    width: u32,
    height: u32,
    fps: f64,
}

struct Session<'a> {
    api: &'a Api,
    context: *mut c_void,
    list: *mut c_void,
    device: *mut c_void,
    pipeline: *mut c_void,
    profiles: Vec<*mut c_void>,
    config: *mut c_void,
    frameset: *mut c_void,
    frames: Vec<*mut c_void>,
    started: bool,
    serial: String,
    model: String,
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
        let api = unsafe {
            Self {
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
                match_video: symbol(&library, b"ob_stream_profile_list_get_video_stream_profile")?,
                delete_profile: symbol(&library, b"ob_delete_stream_profile")?,
                delete_profile_list: symbol(&library, b"ob_delete_stream_profile_list")?,
                create_config: symbol(&library, b"ob_create_config")?,
                delete_config: symbol(&library, b"ob_delete_config")?,
                enable_profile: symbol(&library, b"ob_config_enable_stream_with_stream_profile")?,
                start_config: symbol(&library, b"ob_pipeline_start_with_config")?,
                stop_pipeline: symbol(&library, b"ob_pipeline_stop")?,
                wait_frames: symbol(&library, b"ob_pipeline_wait_for_frameset")?,
                frame_by_type: symbol(&library, b"ob_frameset_get_frame")?,
                frame_data: symbol(&library, b"ob_frame_get_data")?,
                frame_size: symbol(&library, b"ob_frame_get_data_size")?,
                frame_format: symbol(&library, b"ob_frame_get_format")?,
                frame_index: symbol(&library, b"ob_frame_get_index")?,
                frame_timestamp: symbol(&library, b"ob_frame_get_timestamp_us")?,
                frame_width: symbol(&library, b"ob_video_frame_get_width")?,
                frame_height: symbol(&library, b"ob_video_frame_get_height")?,
                depth_scale: symbol(&library, b"ob_depth_frame_get_value_scale")?,
                profile_width: symbol(&library, b"ob_video_stream_profile_get_width")?,
                profile_height: symbol(&library, b"ob_video_stream_profile_get_height")?,
                profile_fps: symbol(&library, b"ob_video_stream_profile_get_fps")?,
                intrinsic: symbol(&library, b"ob_video_stream_profile_get_intrinsic")?,
                distortion: symbol(&library, b"ob_video_stream_profile_get_distortion")?,
                extrinsic: symbol(&library, b"ob_stream_profile_get_extrinsic_to")?,
                delete_frame: symbol(&library, b"ob_delete_frame")?,
                error_message: symbol(&library, b"ob_error_get_message")?,
                delete_error: symbol(&library, b"ob_delete_error")?,
                set_extensions: symbol(&library, b"ob_set_extensions_directory")?,
                device_info: optional(&library, b"ob_device_get_device_info"),
                info_name: optional(&library, b"ob_device_info_get_name"),
                delete_info: optional(&library, b"ob_delete_device_info"),
                _library: library,
            }
        };
        api.check_version()?;
        Ok(api)
    }

    fn check_version(&self) -> Result<(), OrbbecCaptureError> {
        let (major, minor, patch) =
            unsafe { ((self.get_major)(), (self.get_minor)(), (self.get_patch)()) };
        let version = format!("{major}.{minor}.{patch}");
        if version == ORBBEC_SDK_VERSION {
            Ok(())
        } else {
            Err(OrbbecCaptureError::Sdk(format!(
                "loaded Orbbec SDK {version}, expected {ORBBEC_SDK_VERSION}"
            )))
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

impl<'a> Session<'a> {
    fn new(api: &'a Api) -> Self {
        Self {
            api,
            context: ptr::null_mut(),
            list: ptr::null_mut(),
            device: ptr::null_mut(),
            pipeline: ptr::null_mut(),
            profiles: Vec::new(),
            config: ptr::null_mut(),
            frameset: ptr::null_mut(),
            frames: Vec::new(),
            started: false,
            serial: String::new(),
            model: "orbbec".to_owned(),
        }
    }

    fn open(&mut self, serial: Option<&str>) -> Result<(), OrbbecCaptureError> {
        self.prepare_extensions()?;
        self.open_context()?;
        let index = self.select(serial)?;
        self.serial = self.serial_at(index)?;
        self.open_pipeline(index)?;
        self.model = self.model_name();
        Ok(())
    }

    fn view(
        &mut self,
        read_frames: bool,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<DeviceView, OrbbecCaptureError> {
        let matched = self.match_streams()?;
        let planes = if read_frames {
            self.start(&matched)?;
            self.read_planes(&matched, timeout, max_bytes)?
        } else {
            matched.iter().map(profile_plane).collect()
        };
        if planes.is_empty() {
            return Err(OrbbecCaptureError::Sdk(
                "device advertised no frame-set channels".to_owned(),
            ));
        }
        Ok(DeviceView {
            serial: self.serial.clone(),
            model: self.model.clone(),
            planes,
        })
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
        let mut serials = Vec::new();
        for index in 0..count {
            serials.push(self.serial_at(index)?);
        }
        let chosen = select_device(&serials, serial)?;
        u32::try_from(chosen)
            .map_err(|_| OrbbecCaptureError::Sdk("device index overflow".to_owned()))
    }

    fn serial_at(&self, index: u32) -> Result<String, OrbbecCaptureError> {
        let mut error = ptr::null_mut();
        let value = unsafe { (self.api.list_serial)(self.list, index, &raw mut error) };
        self.api.check(error)?;
        if value.is_null() {
            return Ok(String::new());
        }
        Ok(unsafe { CStr::from_ptr(value) }
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

    fn model_name(&self) -> String {
        let (Some(get_info), Some(get_name), Some(delete_info)) = (
            self.api.device_info,
            self.api.info_name,
            self.api.delete_info,
        ) else {
            return "orbbec".to_owned();
        };
        let mut error = ptr::null_mut();
        let info = unsafe { get_info(self.device, &raw mut error) };
        if self.api.consume_error(error).is_some() || info.is_null() {
            return "orbbec".to_owned();
        }
        let mut error = ptr::null_mut();
        let name = unsafe { get_name(info, &raw mut error) };
        let model = if self.api.consume_error(error).is_some() || name.is_null() {
            "orbbec".to_owned()
        } else {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        };
        let mut error = ptr::null_mut();
        unsafe { delete_info(info, &raw mut error) };
        let _ = self.api.consume_error(error);
        if model.is_empty() {
            "orbbec".to_owned()
        } else {
            model
        }
    }

    fn match_streams(&mut self) -> Result<Vec<Matched>, OrbbecCaptureError> {
        let mut matched = Vec::new();
        for spec in STREAMS {
            if let Some(item) = self.match_spec(spec)? {
                matched.push(item);
            }
        }
        let has_stereo = matched
            .iter()
            .any(|item| item.role == "infrared-left" || item.role == "infrared-right");
        if !has_stereo && let Some(item) = self.match_spec(&INFRARED)? {
            matched.push(item);
        }
        if !matched.iter().any(|item| item.role == "color") {
            return Err(OrbbecCaptureError::NoColorProfile(
                "no MJPEG color profile".to_owned(),
            ));
        }
        Ok(matched)
    }

    fn match_spec(&mut self, spec: &StreamSpec) -> Result<Option<Matched>, OrbbecCaptureError> {
        let Some(list) = self.profile_list(spec)? else {
            return Ok(None);
        };
        let found = self.first_profile(list, spec);
        self.delete_list(list);
        let Some((profile, format)) = found? else {
            return Ok(None);
        };
        let (width, height, fps) = self.profile_shape(profile);
        if width == 0 || height == 0 {
            self.profiles.push(profile);
            return Ok(None);
        }
        self.profiles.push(profile);
        Ok(Some(Matched {
            frame_type: spec.frame_type,
            channel_id: spec.channel_id,
            role: spec.role,
            format,
            profile,
            width,
            height,
            fps,
        }))
    }

    fn profile_list(&self, spec: &StreamSpec) -> Result<Option<*mut c_void>, OrbbecCaptureError> {
        let mut error = ptr::null_mut();
        let list = unsafe { (self.api.profile_list)(self.pipeline, spec.sensor, &raw mut error) };
        if let Some(message) = self.api.consume_error(error) {
            return if spec.required {
                Err(OrbbecCaptureError::NoColorProfile(message))
            } else {
                Ok(None)
            };
        }
        if list.is_null() {
            return if spec.required {
                Err(OrbbecCaptureError::NoColorProfile(
                    "color sensor profile list was empty".to_owned(),
                ))
            } else {
                Ok(None)
            };
        }
        Ok(Some(list))
    }

    fn first_profile(
        &self,
        list: *mut c_void,
        spec: &StreamSpec,
    ) -> Result<Option<(*mut c_void, i32)>, OrbbecCaptureError> {
        for format in spec.formats {
            let mut error = ptr::null_mut();
            let profile = unsafe { (self.api.match_video)(list, 0, 0, *format, 0, &raw mut error) };
            if let Some(message) = self.api.consume_error(error) {
                if spec.required {
                    return Err(OrbbecCaptureError::NoColorProfile(message));
                }
                continue;
            }
            if !profile.is_null() {
                return Ok(Some((profile, *format)));
            }
        }
        if spec.required {
            Err(OrbbecCaptureError::NoColorProfile(
                "no MJPEG color profile".to_owned(),
            ))
        } else {
            Ok(None)
        }
    }

    fn profile_shape(&self, profile: *mut c_void) -> (u32, u32, f64) {
        let width = self.u32_profile(self.api.profile_width, profile);
        let height = self.u32_profile(self.api.profile_height, profile);
        let fps = f64::from(self.u32_profile(self.api.profile_fps, profile));
        (width, height, if fps > 0.0 { fps } else { 30.0 })
    }

    fn u32_profile(&self, function: ProfileU32, profile: *const c_void) -> u32 {
        let mut error = ptr::null_mut();
        let value = unsafe { function(profile, &raw mut error) };
        if self.api.consume_error(error).is_some() {
            0
        } else {
            value
        }
    }

    fn start(&mut self, matched: &[Matched]) -> Result<(), OrbbecCaptureError> {
        let mut error = ptr::null_mut();
        let config = unsafe { (self.api.create_config)(&raw mut error) };
        self.api.check(error)?;
        if config.is_null() {
            return Err(OrbbecCaptureError::Sdk(
                "pipeline config was not created".to_owned(),
            ));
        }
        self.config = config;
        for item in matched {
            let mut error = ptr::null_mut();
            unsafe { (self.api.enable_profile)(config, item.profile, &raw mut error) };
            self.api.check(error)?;
        }
        let mut error = ptr::null_mut();
        unsafe { (self.api.start_config)(self.pipeline, config, &raw mut error) };
        self.api.check(error)?;
        self.started = true;
        Ok(())
    }

    fn read_planes(
        &mut self,
        matched: &[Matched],
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Vec<Plane>, OrbbecCaptureError> {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let mut error = ptr::null_mut();
        let frameset = unsafe { (self.api.wait_frames)(self.pipeline, millis, &raw mut error) };
        self.api.check(error)?;
        if frameset.is_null() {
            return Err(OrbbecCaptureError::Timeout);
        }
        self.frameset = frameset;
        let color = matched
            .iter()
            .find(|item| item.role == "color")
            .map(|item| item.profile);
        let mut planes = Vec::with_capacity(matched.len());
        for item in matched {
            planes.push(self.read_plane(item, color, max_bytes)?);
        }
        Ok(planes)
    }

    fn read_plane(
        &mut self,
        item: &Matched,
        color: Option<*mut c_void>,
        max_bytes: u64,
    ) -> Result<Plane, OrbbecCaptureError> {
        let mut error = ptr::null_mut();
        let frame =
            unsafe { (self.api.frame_by_type)(self.frameset, item.frame_type, &raw mut error) };
        self.api.check(error)?;
        if frame.is_null() {
            return Err(OrbbecCaptureError::Timeout);
        }
        self.frames.push(frame);
        let format = self
            .frame_i32(self.api.frame_format, frame)
            .unwrap_or(item.format);
        if format != item.format {
            return Err(OrbbecCaptureError::Malformed(format!(
                "{} frame format {format} did not match the enabled profile",
                item.channel_id
            )));
        }
        let width = self
            .frame_u32(self.api.frame_width, frame)
            .unwrap_or(item.width);
        let height = self
            .frame_u32(self.api.frame_height, frame)
            .unwrap_or(item.height);
        let samples = self.copy_frame(frame, max_bytes)?;
        Ok(Plane {
            channel_id: item.channel_id,
            role: item.role,
            format: plane_format(format)?,
            width,
            height,
            fps: item.fps,
            timestamp_us: self.frame_u64(self.api.frame_timestamp, frame).unwrap_or(0),
            frame_number: self.frame_u64(self.api.frame_index, frame).unwrap_or(0),
            depth_scale_mm: self.depth_scale(item, frame),
            samples,
            intrinsics: self.intrinsics(item.profile),
            distortion: self.distortion(item.profile),
            rotation: self.extrinsic(item, color).map(|(rotation, _)| rotation),
            translation: self
                .extrinsic(item, color)
                .map(|(_, translation)| translation),
        })
    }

    fn depth_scale(&self, item: &Matched, frame: *mut c_void) -> Option<f32> {
        if item.role != "depth" {
            return None;
        }
        let mut error = ptr::null_mut();
        let scale = unsafe { (self.api.depth_scale)(frame, &raw mut error) };
        if self.api.consume_error(error).is_some() || !scale.is_finite() || scale <= 0.0 {
            None
        } else {
            Some(scale)
        }
    }

    fn copy_frame(
        &self,
        frame: *mut c_void,
        max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
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

    fn intrinsics(&self, profile: *mut c_void) -> Option<CameraIntrinsics> {
        let mut error = ptr::null_mut();
        let value = unsafe { (self.api.intrinsic)(profile, &raw mut error) };
        if self.api.consume_error(error).is_some() {
            return None;
        }
        let width = u32::try_from(value.width).unwrap_or(0);
        let height = u32::try_from(value.height).unwrap_or(0);
        if width == 0 || height == 0 || value.fx == 0.0 || value.fy == 0.0 {
            return None;
        }
        Some(CameraIntrinsics {
            fx: value.fx,
            fy: value.fy,
            cx: value.cx,
            cy: value.cy,
            width,
            height,
        })
    }

    fn distortion(&self, profile: *mut c_void) -> Option<(String, Vec<f32>)> {
        let mut error = ptr::null_mut();
        let value = unsafe { (self.api.distortion)(profile, &raw mut error) };
        if self.api.consume_error(error).is_some() {
            return None;
        }
        let coefficients = [
            value.k1, value.k2, value.k3, value.k4, value.k5, value.k6, value.p1, value.p2,
        ];
        if coefficients.iter().any(|item| !item.is_finite()) {
            return None;
        }
        Some((
            distortion_name(value.model).to_owned(),
            coefficients.to_vec(),
        ))
    }

    fn extrinsic(
        &self,
        item: &Matched,
        color: Option<*mut c_void>,
    ) -> Option<([f32; 9], [f32; 3])> {
        if item.role == "color" {
            return None;
        }
        let color = color?;
        let mut error = ptr::null_mut();
        let value = unsafe { (self.api.extrinsic)(item.profile, color, &raw mut error) };
        if self.api.consume_error(error).is_some() {
            return None;
        }
        if value
            .rot
            .iter()
            .chain(value.trans.iter())
            .any(|item| !item.is_finite())
        {
            return None;
        }
        Some((value.rot, value.trans))
    }

    fn frame_u64(&self, function: FrameU64, frame: *const c_void) -> Option<u64> {
        let mut error = ptr::null_mut();
        let value = unsafe { function(frame, &raw mut error) };
        if self.api.consume_error(error).is_some() {
            None
        } else {
            Some(value)
        }
    }

    fn frame_u32(&self, function: FrameU32, frame: *const c_void) -> Option<u32> {
        let mut error = ptr::null_mut();
        let value = unsafe { function(frame, &raw mut error) };
        if self.api.consume_error(error).is_some() {
            None
        } else {
            Some(value)
        }
    }

    fn frame_i32(&self, function: FrameFormat, frame: *const c_void) -> Option<i32> {
        let mut error = ptr::null_mut();
        let value = unsafe { function(frame, &raw mut error) };
        if self.api.consume_error(error).is_some() {
            None
        } else {
            Some(value)
        }
    }

    fn delete_list(&self, list: *mut c_void) {
        let mut error = ptr::null_mut();
        unsafe { (self.api.delete_profile_list)(list, &raw mut error) };
        let _ = self.api.consume_error(error);
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        let frames = std::mem::take(&mut self.frames);
        let frameset = self.frameset;
        self.frameset = ptr::null_mut();
        for frame in frames {
            self.release_frame(frame);
        }
        self.release_frame(frameset);
        if self.started && !self.pipeline.is_null() {
            let mut error = ptr::null_mut();
            unsafe { (self.api.stop_pipeline)(self.pipeline, &raw mut error) };
            let _ = self.api.consume_error(error);
        }
        let profiles = std::mem::take(&mut self.profiles);
        for profile in profiles {
            let mut error = ptr::null_mut();
            unsafe { (self.api.delete_profile)(profile, &raw mut error) };
            let _ = self.api.consume_error(error);
        }
        self.release_config();
        self.release_pipeline();
        self.release_device();
        self.release_list();
        self.release_context();
    }
}

impl Session<'_> {
    fn release_frame(&self, frame: *mut c_void) {
        if frame.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        unsafe { (self.api.delete_frame)(frame, &raw mut error) };
        let _ = self.api.consume_error(error);
    }

    fn release_config(&self) {
        if self.config.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        unsafe { (self.api.delete_config)(self.config, &raw mut error) };
        let _ = self.api.consume_error(error);
    }

    fn release_pipeline(&self) {
        if self.pipeline.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        unsafe { (self.api.delete_pipeline)(self.pipeline, &raw mut error) };
        let _ = self.api.consume_error(error);
    }

    fn release_device(&self) {
        if self.device.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        unsafe { (self.api.delete_device)(self.device, &raw mut error) };
        let _ = self.api.consume_error(error);
    }

    fn release_list(&self) {
        if self.list.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        unsafe { (self.api.delete_list)(self.list, &raw mut error) };
        let _ = self.api.consume_error(error);
    }

    fn release_context(&self) {
        if self.context.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        unsafe { (self.api.delete_context)(self.context, &raw mut error) };
        let _ = self.api.consume_error(error);
    }
}

fn profile_plane(item: &Matched) -> Plane {
    Plane {
        channel_id: item.channel_id,
        role: item.role,
        format: plane_format(item.format).unwrap_or(PlaneFormat::Gray8),
        width: item.width,
        height: item.height,
        fps: item.fps,
        timestamp_us: 0,
        frame_number: 0,
        depth_scale_mm: None,
        samples: Vec::new(),
        intrinsics: None,
        distortion: None,
        rotation: None,
        translation: None,
    }
}

fn plane_format(format: i32) -> Result<PlaneFormat, OrbbecCaptureError> {
    match format {
        FORMAT_MJPG => Ok(PlaneFormat::Jpeg),
        FORMAT_Y8 => Ok(PlaneFormat::Gray8),
        FORMAT_Y16 => Ok(PlaneFormat::Gray16Le),
        other => Err(OrbbecCaptureError::Malformed(format!(
            "frame format {other} is not MJPEG, Y8, or Y16"
        ))),
    }
}

fn distortion_name(model: i32) -> &'static str {
    match model {
        0 => "none",
        1 => "modified-brown-conrady",
        2 => "inverse-brown-conrady",
        3 => "brown-conrady",
        4 => "brown-conrady-k6",
        5 => "kannala-brandt4",
        _ => "unknown",
    }
}

fn permission(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("permission")
        || lower.contains("access denied")
        || lower.contains("operation not permitted")
        || lower.contains("libusb_error_access")
        || lower.contains("errno=13")
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

fn optional<T: Copy>(library: &libloading::Library, name: &[u8]) -> Option<T> {
    unsafe { symbol(library, name).ok() }
}
