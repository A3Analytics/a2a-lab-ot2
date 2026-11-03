//! Vendor-neutral frame-set values.

use std::collections::BTreeSet;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::error::FrameSetError;
use super::id::{FrameChannelId, FrameSetId, FrameSetSourceId};
use super::payload::{jpeg_ok, png_info};

const COLOR: &str = "color";
const DEPTH: &str = "depth";
const INFRARED_LEFT: &str = "infrared-left";
const INFRARED_RIGHT: &str = "infrared-right";
const CONFIDENCE: &str = "confidence";

/// Extensible channel role stored as one validated string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelRole(String);

impl ChannelRole {
    /// Color image.
    #[must_use]
    pub fn color() -> Self {
        Self(COLOR.to_owned())
    }

    /// Depth image.
    #[must_use]
    pub fn depth() -> Self {
        Self(DEPTH.to_owned())
    }

    /// Left infrared image.
    #[must_use]
    pub fn infrared_left() -> Self {
        Self(INFRARED_LEFT.to_owned())
    }

    /// Right infrared image.
    #[must_use]
    pub fn infrared_right() -> Self {
        Self(INFRARED_RIGHT.to_owned())
    }

    /// Capture confidence image.
    #[must_use]
    pub fn confidence() -> Self {
        Self(CONFIDENCE.to_owned())
    }

    /// Validates a known or custom role.
    pub fn new(value: impl AsRef<str>) -> Result<Self, FrameSetError> {
        let value = value.as_ref();
        if !valid_role(value) {
            return Err(FrameSetError::invalid(
                "channel_role",
                "must be 1..=64 lowercase ASCII letters, digits, or hyphens",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Role text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this role is depth.
    #[must_use]
    pub fn is_depth(&self) -> bool {
        self.0 == DEPTH
    }

    /// Whether this role is color.
    #[must_use]
    pub fn is_color(&self) -> bool {
        self.0 == COLOR
    }
}

fn valid_role(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// One advertised channel profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelProfile {
    id: FrameChannelId,
    role: ChannelRole,
    media_type: String,
    width: u32,
    height: u32,
    bit_depth: u8,
    frame_rate_hz: f64,
}

impl ChannelProfile {
    /// Validates a profile independent of any vendor.
    pub fn new(
        id: FrameChannelId,
        role: ChannelRole,
        media_type: impl Into<String>,
        width: u32,
        height: u32,
        bit_depth: u8,
        frame_rate_hz: f64,
    ) -> Result<Self, FrameSetError> {
        let media_type = media_type.into();
        check_profile(&role, &media_type, width, height, bit_depth, frame_rate_hz)?;
        Ok(Self {
            id,
            role,
            media_type,
            width,
            height,
            bit_depth,
            frame_rate_hz,
        })
    }

    /// Channel identifier.
    #[must_use]
    pub const fn id(&self) -> &FrameChannelId {
        &self.id
    }

    /// Channel role.
    #[must_use]
    pub const fn role(&self) -> &ChannelRole {
        &self.role
    }

    /// Payload media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Native bit depth.
    #[must_use]
    pub const fn bit_depth(&self) -> u8 {
        self.bit_depth
    }

    /// Advertised frame rate.
    #[must_use]
    pub const fn frame_rate_hz(&self) -> f64 {
        self.frame_rate_hz
    }
}

fn check_profile(
    role: &ChannelRole,
    media_type: &str,
    width: u32,
    height: u32,
    bit_depth: u8,
    frame_rate_hz: f64,
) -> Result<(), FrameSetError> {
    if width == 0 || height == 0 {
        return Err(FrameSetError::invalid(
            "dimensions",
            "width and height must be positive",
        ));
    }
    if !frame_rate_hz.is_finite() || frame_rate_hz <= 0.0 {
        return Err(FrameSetError::invalid(
            "frame_rate_hz",
            "must be a positive finite number",
        ));
    }
    let expected = if role.is_color() {
        media_type == "image/jpeg" && bit_depth == 8
    } else if role.is_depth() {
        media_type == "image/png" && bit_depth == 16
    } else {
        media_type == "image/png" && matches!(bit_depth, 8 | 16)
    };
    if !expected {
        return Err(FrameSetError::invalid(
            "profile",
            "color is 8-bit image/jpeg, depth is 16-bit image/png, and other channels are 8-bit or 16-bit image/png",
        ));
    }
    Ok(())
}

/// Device and profile description for one synchronized source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameSetSource {
    id: FrameSetSourceId,
    backend: String,
    serial: String,
    model: String,
    channels: Vec<ChannelProfile>,
    sync_group: String,
    sync_tolerance_us: u64,
}

impl FrameSetSource {
    /// Validates device identity and channel profiles.
    pub fn new(
        id: FrameSetSourceId,
        backend: impl Into<String>,
        serial: impl Into<String>,
        model: impl Into<String>,
        channels: Vec<ChannelProfile>,
        sync_group: impl Into<String>,
        sync_tolerance_us: u64,
    ) -> Result<Self, FrameSetError> {
        let backend = nonempty("backend", backend.into())?;
        let serial = nonempty("serial", serial.into())?;
        let model = nonempty("model", model.into())?;
        let sync_group = nonempty("sync_group", sync_group.into())?;
        if channels.is_empty() {
            return Err(FrameSetError::invalid(
                "channels",
                "a source needs at least one channel",
            ));
        }
        unique_channels(&channels)?;
        Ok(Self {
            id,
            backend,
            serial,
            model,
            channels,
            sync_group,
            sync_tolerance_us,
        })
    }

    /// Source identifier.
    #[must_use]
    pub const fn id(&self) -> &FrameSetSourceId {
        &self.id
    }

    /// Backend name. This is not a vendor-specific schema.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Device serial.
    #[must_use]
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// Device model.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Advertised channels.
    #[must_use]
    pub fn channels(&self) -> &[ChannelProfile] {
        &self.channels
    }

    /// Synchronization group shared by this device's channels.
    #[must_use]
    pub fn sync_group(&self) -> &str {
        &self.sync_group
    }

    /// Maximum device-timestamp spread for one set, in microseconds.
    #[must_use]
    pub const fn sync_tolerance_us(&self) -> u64 {
        self.sync_tolerance_us
    }

    pub(crate) fn profile(&self, id: &FrameChannelId) -> Option<&ChannelProfile> {
        self.channels.iter().find(|profile| profile.id() == id)
    }
}

fn nonempty(field: &'static str, value: String) -> Result<String, FrameSetError> {
    if value.is_empty() || value.len() > 256 {
        return Err(FrameSetError::invalid(field, "must be 1..=256 characters"));
    }
    Ok(value)
}

fn unique_channels(channels: &[ChannelProfile]) -> Result<(), FrameSetError> {
    let mut ids = BTreeSet::new();
    let mut roles = BTreeSet::new();
    for channel in channels {
        if !ids.insert(channel.id().as_str()) {
            return Err(FrameSetError::invalid(
                "channels",
                "channel identifiers must be unique",
            ));
        }
        if !roles.insert(channel.role().as_str()) {
            return Err(FrameSetError::invalid(
                "channels",
                "channel roles must be unique",
            ));
        }
    }
    Ok(())
}

/// Pinhole intrinsics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intrinsics {
    /// Focal length in x pixels.
    pub fx: f64,
    /// Focal length in y pixels.
    pub fy: f64,
    /// Principal point x.
    pub cx: f64,
    /// Principal point y.
    pub cy: f64,
    /// Calibration width.
    pub width: u32,
    /// Calibration height.
    pub height: u32,
}

impl Intrinsics {
    /// Rejects non-finite values and empty dimensions.
    pub fn new(
        fx: f64,
        fy: f64,
        cx: f64,
        cy: f64,
        width: u32,
        height: u32,
    ) -> Result<Self, FrameSetError> {
        if !fx.is_finite()
            || !fy.is_finite()
            || fx == 0.0
            || fy == 0.0
            || !cx.is_finite()
            || !cy.is_finite()
        {
            return Err(FrameSetError::invalid(
                "intrinsics",
                "focal lengths and principal point must be finite, and focal lengths must be non-zero",
            ));
        }
        if width == 0 || height == 0 {
            return Err(FrameSetError::invalid(
                "intrinsics",
                "width and height must be positive",
            ));
        }
        Ok(Self {
            fx,
            fy,
            cx,
            cy,
            width,
            height,
        })
    }
}

/// Distortion model and finite coefficients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Distortion {
    model: String,
    coefficients: Vec<f64>,
}

impl Distortion {
    /// Validates a named distortion model.
    pub fn new(model: impl Into<String>, coefficients: Vec<f64>) -> Result<Self, FrameSetError> {
        let model = nonempty("distortion", model.into())?;
        if coefficients.is_empty() || coefficients.iter().any(|value| !value.is_finite()) {
            return Err(FrameSetError::invalid(
                "distortion",
                "coefficients must be a non-empty list of finite numbers",
            ));
        }
        Ok(Self {
            model,
            coefficients,
        })
    }

    /// Distortion model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Model coefficients.
    #[must_use]
    pub fn coefficients(&self) -> &[f64] {
        &self.coefficients
    }
}

/// Rigid transform from this channel to a reference channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extrinsics {
    rotation: [f64; 9],
    translation: [f64; 3],
    reference_channel: FrameChannelId,
}

impl Extrinsics {
    /// Validates a finite transform.
    pub fn new(
        rotation: [f64; 9],
        translation: [f64; 3],
        reference_channel: FrameChannelId,
    ) -> Result<Self, FrameSetError> {
        if rotation
            .iter()
            .chain(translation.iter())
            .any(|value| !value.is_finite())
        {
            return Err(FrameSetError::invalid(
                "extrinsics",
                "rotation and translation must be finite",
            ));
        }
        Ok(Self {
            rotation,
            translation,
            reference_channel,
        })
    }

    /// Row-major rotation.
    #[must_use]
    pub const fn rotation(&self) -> &[f64; 9] {
        &self.rotation
    }

    /// Translation in the depth unit of the source.
    #[must_use]
    pub const fn translation(&self) -> &[f64; 3] {
        &self.translation
    }

    /// Channel this transform targets.
    #[must_use]
    pub const fn reference_channel(&self) -> &FrameChannelId {
        &self.reference_channel
    }
}

/// How to convert a depth sample into a physical distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepthUnit {
    /// Sample times scale is millimeters.
    Millimeter,
    /// Sample times scale is meters.
    Meter,
}

/// Positive depth scale and its unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepthScale {
    scale: f64,
    units: DepthUnit,
}

impl DepthScale {
    /// Rejects a non-positive or non-finite scale.
    pub fn new(scale: f64, units: DepthUnit) -> Result<Self, FrameSetError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(FrameSetError::invalid(
                "depth_scale",
                "must be a positive finite number",
            ));
        }
        Ok(Self { scale, units })
    }

    /// Multiplier from sample to [`Self::units`].
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// Physical unit of `sample * scale`.
    #[must_use]
    pub const fn units(&self) -> DepthUnit {
        self.units
    }
}

/// Optional geometric calibration for one channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelCalibration {
    intrinsics: Option<Intrinsics>,
    distortion: Option<Distortion>,
    extrinsics: Option<Extrinsics>,
}

impl ChannelCalibration {
    /// Builds calibration from already validated parts.
    #[must_use]
    pub fn new(
        intrinsics: Option<Intrinsics>,
        distortion: Option<Distortion>,
        extrinsics: Option<Extrinsics>,
    ) -> Self {
        Self {
            intrinsics,
            distortion,
            extrinsics,
        }
    }

    /// Intrinsics when the device reported them.
    #[must_use]
    pub const fn intrinsics(&self) -> Option<&Intrinsics> {
        self.intrinsics.as_ref()
    }

    /// Distortion when the device reported it.
    #[must_use]
    pub const fn distortion(&self) -> Option<&Distortion> {
        self.distortion.as_ref()
    }

    /// Extrinsics when the device reported them.
    #[must_use]
    pub const fn extrinsics(&self) -> Option<&Extrinsics> {
        self.extrinsics.as_ref()
    }
}

impl Default for ChannelCalibration {
    fn default() -> Self {
        Self::new(None, None, None)
    }
}

/// One channel inside a synchronized set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameChannel {
    id: FrameChannelId,
    role: ChannelRole,
    media_type: String,
    width: u32,
    height: u32,
    bit_depth: u8,
    device_timestamp_us: u64,
    frame_number: u64,
    depth: Option<DepthScale>,
    calibration: ChannelCalibration,
    attributes: Map<String, Value>,
    data: Vec<u8>,
}

impl FrameChannel {
    /// Channel identifier.
    #[must_use]
    pub const fn id(&self) -> &FrameChannelId {
        &self.id
    }

    /// Channel role.
    #[must_use]
    pub const fn role(&self) -> &ChannelRole {
        &self.role
    }

    /// Payload media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Native bit depth.
    #[must_use]
    pub const fn bit_depth(&self) -> u8 {
        self.bit_depth
    }

    /// Device clock timestamp in microseconds.
    #[must_use]
    pub const fn device_timestamp_us(&self) -> u64 {
        self.device_timestamp_us
    }

    /// Device frame number.
    #[must_use]
    pub const fn frame_number(&self) -> u64 {
        self.frame_number
    }

    /// Depth scale when this channel is depth.
    #[must_use]
    pub const fn depth(&self) -> Option<&DepthScale> {
        self.depth.as_ref()
    }

    /// Geometric calibration.
    #[must_use]
    pub const fn calibration(&self) -> &ChannelCalibration {
        &self.calibration
    }

    /// Generic diagnostics. These do not carry pixel bytes.
    #[must_use]
    pub const fn attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }

    /// Encoded payload.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// Metadata returned by list and search. It contains no payload bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameSetMetadata {
    id: FrameSetId,
    source_id: FrameSetSourceId,
    captured_at: String,
    channels: Vec<ChannelMetadata>,
    attributes: Map<String, Value>,
}

/// One channel without its payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMetadata {
    id: FrameChannelId,
    role: ChannelRole,
    media_type: String,
    width: u32,
    height: u32,
    bit_depth: u8,
    device_timestamp_us: u64,
    frame_number: u64,
    depth: Option<DepthScale>,
    calibration: ChannelCalibration,
    attributes: Map<String, Value>,
    byte_len: u64,
}

impl FrameSetMetadata {
    /// Frame-set identifier.
    #[must_use]
    pub const fn id(&self) -> &FrameSetId {
        &self.id
    }

    /// Source that produced the set.
    #[must_use]
    pub const fn source_id(&self) -> &FrameSetSourceId {
        &self.source_id
    }

    /// Host capture time as RFC 3339 UTC.
    #[must_use]
    pub fn captured_at(&self) -> &str {
        &self.captured_at
    }

    /// Channel metadata.
    #[must_use]
    pub fn channels(&self) -> &[ChannelMetadata] {
        &self.channels
    }

    /// Generic set diagnostics.
    #[must_use]
    pub const fn attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }
}

impl ChannelMetadata {
    /// Channel identifier.
    #[must_use]
    pub const fn id(&self) -> &FrameChannelId {
        &self.id
    }

    /// Channel role.
    #[must_use]
    pub const fn role(&self) -> &ChannelRole {
        &self.role
    }

    /// Payload media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Encoded payload length.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    /// Depth scale when this channel is depth.
    #[must_use]
    pub const fn depth(&self) -> Option<&DepthScale> {
        self.depth.as_ref()
    }

    /// Geometric calibration.
    #[must_use]
    pub const fn calibration(&self) -> &ChannelCalibration {
        &self.calibration
    }

    /// Device timestamp in microseconds.
    #[must_use]
    pub const fn device_timestamp_us(&self) -> u64 {
        self.device_timestamp_us
    }

    /// Device frame number.
    #[must_use]
    pub const fn frame_number(&self) -> u64 {
        self.frame_number
    }
}

/// One atomic synchronized capture before or after publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameSet {
    id: FrameSetId,
    source_id: FrameSetSourceId,
    captured_at: String,
    channels: Vec<FrameChannel>,
    attributes: Map<String, Value>,
}

impl FrameSet {
    /// Validates a complete set against its source and requested channels.
    pub fn new(
        id: FrameSetId,
        source: &FrameSetSource,
        captured_at: impl Into<String>,
        channels: Vec<FrameChannel>,
        requested: &[FrameChannelId],
        attributes: Map<String, Value>,
        max_channel_bytes: u64,
    ) -> Result<Self, FrameSetError> {
        let captured_at = captured_at.into();
        if !valid_timestamp(&captured_at) {
            return Err(FrameSetError::invalid(
                "captured_at",
                "must be an RFC 3339 UTC timestamp",
            ));
        }
        check_attributes(&attributes)?;
        let set = Self {
            id,
            source_id: source.id().clone(),
            captured_at,
            channels,
            attributes,
        };
        set.check_against(source, requested, max_channel_bytes)?;
        Ok(set)
    }

    /// Frame-set identifier.
    #[must_use]
    pub const fn id(&self) -> &FrameSetId {
        &self.id
    }

    /// Source identifier.
    #[must_use]
    pub const fn source_id(&self) -> &FrameSetSourceId {
        &self.source_id
    }

    /// Host capture time.
    #[must_use]
    pub fn captured_at(&self) -> &str {
        &self.captured_at
    }

    /// Channels in request order.
    #[must_use]
    pub fn channels(&self) -> &[FrameChannel] {
        &self.channels
    }

    /// Generic diagnostics.
    #[must_use]
    pub const fn attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }

    /// Metadata view without payload bytes.
    #[must_use]
    pub fn metadata(&self) -> FrameSetMetadata {
        FrameSetMetadata {
            id: self.id.clone(),
            source_id: self.source_id.clone(),
            captured_at: self.captured_at.clone(),
            channels: self
                .channels
                .iter()
                .map(|channel| ChannelMetadata {
                    id: channel.id.clone(),
                    role: channel.role.clone(),
                    media_type: channel.media_type.clone(),
                    width: channel.width,
                    height: channel.height,
                    bit_depth: channel.bit_depth,
                    device_timestamp_us: channel.device_timestamp_us,
                    frame_number: channel.frame_number,
                    depth: channel.depth.clone(),
                    calibration: channel.calibration.clone(),
                    attributes: channel.attributes.clone(),
                    byte_len: u64::try_from(channel.data.len()).unwrap_or(u64::MAX),
                })
                .collect(),
            attributes: self.attributes.clone(),
        }
    }

    /// One channel by identifier.
    #[must_use]
    pub fn channel(&self, id: &FrameChannelId) -> Option<&FrameChannel> {
        self.channels.iter().find(|channel| channel.id() == id)
    }

    fn check_against(
        &self,
        source: &FrameSetSource,
        requested: &[FrameChannelId],
        max_channel_bytes: u64,
    ) -> Result<(), FrameSetError> {
        if self.source_id != *source.id() {
            return Err(FrameSetError::invalid(
                "source_id",
                "frame set source does not match the requested source",
            ));
        }
        let expected = expected_ids(source, requested)?;
        if self.channels.len() != expected.len() {
            return Err(FrameSetError::invalid(
                "channels",
                "a frame set must contain every requested channel and no extra channel",
            ));
        }
        let mut seen = BTreeSet::new();
        for (channel, expected_id) in self.channels.iter().zip(expected.iter()) {
            if channel.id() != *expected_id || !seen.insert(channel.id().as_str()) {
                return Err(FrameSetError::invalid(
                    "channels",
                    "channels must be unique and follow the requested profile order",
                ));
            }
            let profile =
                source
                    .profile(channel.id())
                    .ok_or_else(|| FrameSetError::UnsupportedProfile {
                        device: source.serial().to_owned(),
                        detail: format!("channel {} is not advertised", channel.id()),
                    })?;
            check_channel(channel, profile, max_channel_bytes)?;
        }
        check_sync(&self.channels, source.sync_tolerance_us())?;
        Ok(())
    }
}

fn expected_ids<'a>(
    source: &'a FrameSetSource,
    requested: &'a [FrameChannelId],
) -> Result<Vec<&'a FrameChannelId>, FrameSetError> {
    if requested.is_empty() {
        return Ok(source.channels().iter().map(ChannelProfile::id).collect());
    }
    for id in requested {
        if source.profile(id).is_none() {
            return Err(FrameSetError::UnsupportedProfile {
                device: source.serial().to_owned(),
                detail: format!("channel {id} is not advertised"),
            });
        }
    }
    Ok(requested.iter().collect())
}

fn check_channel(
    channel: &FrameChannel,
    profile: &ChannelProfile,
    max_channel_bytes: u64,
) -> Result<(), FrameSetError> {
    if channel.role() != profile.role()
        || channel.media_type() != profile.media_type()
        || channel.width() != profile.width()
        || channel.height() != profile.height()
        || channel.bit_depth() != profile.bit_depth()
    {
        return Err(FrameSetError::invalid(
            "channel",
            format!(
                "channel {} does not match its advertised profile",
                channel.id()
            ),
        ));
    }
    check_attributes(channel.attributes())?;
    let len = u64::try_from(channel.data().len()).unwrap_or(u64::MAX);
    if len == 0 || len > max_channel_bytes {
        return Err(if len == 0 {
            FrameSetError::Malformed(format!("channel {} is empty", channel.id()))
        } else {
            FrameSetError::Oversize
        });
    }
    if channel.role().is_depth() {
        check_depth(channel)?;
    } else if channel.depth().is_some() {
        return Err(FrameSetError::invalid(
            "depth_scale",
            "only a depth channel carries depth scale",
        ));
    }
    check_payload(channel)
}

fn check_depth(channel: &FrameChannel) -> Result<(), FrameSetError> {
    channel.depth().map(|_| ()).ok_or_else(|| {
        FrameSetError::invalid("depth_scale", "a depth channel requires scale and units")
    })
}

fn check_payload(channel: &FrameChannel) -> Result<(), FrameSetError> {
    if channel.role().is_color() {
        if jpeg_ok(channel.data()) {
            return Ok(());
        }
        return Err(FrameSetError::Malformed(format!(
            "channel {} is not a JPEG",
            channel.id()
        )));
    }
    let png = png_info(channel.data())
        .map_err(|_| FrameSetError::Malformed(format!("channel {} is not a PNG", channel.id())))?;
    if png.width != channel.width()
        || png.height != channel.height()
        || png.bit_depth != channel.bit_depth()
        || png.color_type != 0
    {
        return Err(FrameSetError::Malformed(format!(
            "channel {} PNG header does not match its profile",
            channel.id()
        )));
    }
    Ok(())
}

fn check_sync(channels: &[FrameChannel], tolerance: u64) -> Result<(), FrameSetError> {
    let mut min = u64::MAX;
    let mut max = 0;
    for channel in channels {
        min = min.min(channel.device_timestamp_us());
        max = max.max(channel.device_timestamp_us());
    }
    if max.saturating_sub(min) > tolerance {
        return Err(FrameSetError::Unsynchronized(format!(
            "device timestamps span {}us, which is outside the {tolerance}us synchronization tolerance",
            max.saturating_sub(min)
        )));
    }
    Ok(())
}

fn check_attributes(attributes: &Map<String, Value>) -> Result<(), FrameSetError> {
    for (key, value) in attributes {
        if key.is_empty() || key.len() > 64 {
            return Err(FrameSetError::invalid(
                "attributes",
                "keys must be 1..=64 characters",
            ));
        }
        if matches!(value, Value::Array(items) if items.len() > 32)
            || serde_json::to_string(value).map_or(true, |text| text.len() > 1024)
        {
            return Err(FrameSetError::invalid(
                "attributes",
                "diagnostic values must stay within 1024 bytes",
            ));
        }
    }
    Ok(())
}

fn valid_timestamp(value: &str) -> bool {
    value.ends_with('Z') && jiff::Timestamp::from_str(value).is_ok()
}

/// Capture before an identifier is assigned.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedFrameSet {
    source_id: FrameSetSourceId,
    captured_at: String,
    channels: Vec<FrameChannel>,
    attributes: Map<String, Value>,
}

impl CapturedFrameSet {
    /// Builds an unassigned capture. Publication assigns the identifier.
    pub fn new(
        source: &FrameSetSource,
        captured_at: impl Into<String>,
        channels: Vec<FrameChannel>,
        requested: &[FrameChannelId],
        attributes: Map<String, Value>,
        max_channel_bytes: u64,
    ) -> Result<Self, FrameSetError> {
        let probed = FrameSet::new(
            FrameSetId::new("capture-probe")?,
            source,
            captured_at,
            channels.clone(),
            requested,
            attributes.clone(),
            max_channel_bytes,
        )?;
        Ok(Self {
            source_id: probed.source_id,
            captured_at: probed.captured_at,
            channels,
            attributes,
        })
    }

    /// Assigns `id` without changing payloads.
    pub fn publish(
        self,
        id: FrameSetId,
        source: &FrameSetSource,
        requested: &[FrameChannelId],
        max_channel_bytes: u64,
    ) -> Result<FrameSet, FrameSetError> {
        FrameSet::new(
            id,
            source,
            self.captured_at,
            self.channels,
            requested,
            self.attributes,
            max_channel_bytes,
        )
    }
}

/// What a capture asks from one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRequest {
    source_id: FrameSetSourceId,
    channel_ids: Vec<FrameChannelId>,
    timeout_ms: u64,
    max_channel_bytes: u64,
}

impl CaptureRequest {
    /// Validates a capture request.
    pub fn new(
        source_id: FrameSetSourceId,
        channel_ids: Vec<FrameChannelId>,
        timeout_ms: u64,
        max_channel_bytes: u64,
    ) -> Result<Self, FrameSetError> {
        if timeout_ms == 0 || max_channel_bytes == 0 {
            return Err(FrameSetError::invalid(
                "capture",
                "timeout and payload limit must be positive",
            ));
        }
        Ok(Self {
            source_id,
            channel_ids,
            timeout_ms,
            max_channel_bytes,
        })
    }

    /// Source to capture.
    #[must_use]
    pub const fn source_id(&self) -> &FrameSetSourceId {
        &self.source_id
    }

    /// Requested channels. Empty means every advertised channel.
    #[must_use]
    pub fn channel_ids(&self) -> &[FrameChannelId] {
        &self.channel_ids
    }

    /// Capture deadline in milliseconds.
    #[must_use]
    pub const fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Maximum encoded bytes for one channel.
    #[must_use]
    pub const fn max_channel_bytes(&self) -> u64 {
        self.max_channel_bytes
    }
}

/// Inputs a backend uses to build one channel before validation.
pub struct FrameChannelParts {
    /// Channel identifier.
    pub id: FrameChannelId,
    /// Channel role.
    pub role: ChannelRole,
    /// Media type.
    pub media_type: String,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Bit depth.
    pub bit_depth: u8,
    /// Device timestamp.
    pub device_timestamp_us: u64,
    /// Device frame number.
    pub frame_number: u64,
    /// Depth scale for a depth channel.
    pub depth: Option<DepthScale>,
    /// Calibration.
    pub calibration: ChannelCalibration,
    /// Diagnostics.
    pub attributes: Map<String, Value>,
    /// Encoded bytes.
    pub data: Vec<u8>,
}

impl FrameChannelParts {
    pub(crate) fn build(self) -> FrameChannel {
        FrameChannel {
            id: self.id,
            role: self.role,
            media_type: self.media_type,
            width: self.width,
            height: self.height,
            bit_depth: self.bit_depth,
            device_timestamp_us: self.device_timestamp_us,
            frame_number: self.frame_number,
            depth: self.depth,
            calibration: self.calibration,
            attributes: self.attributes,
            data: self.data,
        }
    }
}

/// Builds a channel for a backend or test.
#[must_use]
pub fn frame_channel(parts: FrameChannelParts) -> FrameChannel {
    parts.build()
}
