//! JPEG to H.264 Annex B through `ffmpeg` `libx264`.
//!
//! Each stored sample is one IDR access unit. Codec initialization is the SPS and PPS
//! in Annex B. Sample bytes are length-prefixed NALs for CMAF.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use a2a_lab_dev_kit::A2aLabError;

/// One encoded picture.
pub(crate) struct AccessUnit {
    /// True when the unit contains an IDR slice.
    pub keyframe: bool,
    /// Annex B SPS and PPS, when this unit carries them.
    pub parameter_sets: Vec<u8>,
    /// Length-prefixed NALs for the media sample.
    pub sample: Vec<u8>,
}

/// Long-lived encoder. One JPEG in, one access unit out.
pub(crate) struct H264Encoder {
    engine: EncoderEngine,
}

enum EncoderEngine {
    /// `ffmpeg` libx264. Pictures come out one frame behind.
    Ffmpeg(FfmpegEncoder),
    /// In-process OpenH264 used when `ffmpeg` is not installed.
    Bundled(BundledEncoder),
}

const FFMPEG_ARGS: &[&str] = &[
    "-hide_banner",
    "-loglevel",
    "error",
    "-fflags",
    "nobuffer",
    "-probesize",
    "32",
    "-analyzeduration",
    "0",
    "-f",
    "image2pipe",
    "-c:v",
    "mjpeg",
    "-framerate",
    "5",
    "-i",
    "pipe:0",
    "-an",
    "-c:v",
    "libx264",
    "-threads",
    "1",
    "-preset",
    "veryfast",
    "-tune",
    "zerolatency",
    "-pix_fmt",
    "yuv420p",
    "-profile:v",
    "baseline",
    "-g",
    "1",
    "-keyint_min",
    "1",
    "-bf",
    "0",
    "-x264-params",
    "sliced-threads=0:aud=1",
    "-muxdelay",
    "0",
    "-muxpreload",
    "0",
    "-avioflags",
    "direct",
    "-fflags",
    "+flush_packets",
    "-flush_packets",
    "1",
    "-f",
    "h264",
    "pipe:1",
];

impl H264Encoder {
    /// Starts `ffmpeg` when it is installed, otherwise the bundled OpenH264 encoder.
    pub(crate) fn spawn(fps: u32) -> Result<Self, A2aLabError> {
        if ffmpeg_available() {
            return Ok(Self {
                engine: EncoderEngine::Ffmpeg(spawn_ffmpeg()?),
            });
        }
        Ok(Self {
            engine: EncoderEngine::Bundled(BundledEncoder::new(fps)?),
        })
    }

    /// Writes one JPEG and returns a closed picture when one is ready.
    pub(crate) fn push(&mut self, jpeg: &[u8]) -> Result<Option<AccessUnit>, A2aLabError> {
        match &mut self.engine {
            EncoderEngine::Ffmpeg(encoder) => encoder.push(jpeg),
            EncoderEngine::Bundled(encoder) => encoder.push(jpeg).map(Some),
        }
    }
}

struct FfmpegEncoder {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<AccessUnit>,
    ready: std::collections::VecDeque<AccessUnit>,
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn spawn_ffmpeg() -> Result<FfmpegEncoder, A2aLabError> {
    let mut command = Command::new("ffmpeg");
    command
        .args(FFMPEG_ARGS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    unsafe {
        command.pre_exec(|| {
            // The capture thread already holds camera devices. Drop them in the encoder.
            for fd in 3..256 {
                close(fd);
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| A2aLabError::unavailable(format!("start ffmpeg h264 encoder: {error}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| A2aLabError::unavailable("ffmpeg h264 encoder has no stdin"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| A2aLabError::unavailable("ffmpeg h264 encoder has no stdout"))?;
    let (tx, incoming) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = vec![0_u8; 64 * 1024];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    buffer.extend_from_slice(&chunk[..size]);
                    for unit in drain_access_units(&mut buffer) {
                        if tx.send(unit).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    Ok(FfmpegEncoder {
        child,
        stdin,
        incoming,
        ready: std::collections::VecDeque::new(),
    })
}

impl FfmpegEncoder {
    fn push(&mut self, jpeg: &[u8]) -> Result<Option<AccessUnit>, A2aLabError> {
        self.stdin
            .write_all(jpeg)
            .map_err(|error| encoder_io(&error))?;
        self.stdin.flush().map_err(|error| encoder_io(&error))?;
        while let Ok(unit) = self.incoming.try_recv() {
            self.ready.push_back(unit);
        }
        if let Some(status) = self.child.try_wait().map_err(|error| encoder_io(&error))? {
            return Err(A2aLabError::unavailable(format!(
                "ffmpeg h264 encoder exited ({status})"
            )));
        }
        Ok(self.ready.pop_front())
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        if let EncoderEngine::Ffmpeg(encoder) = &mut self.engine {
            let _ = encoder.child.kill();
            let _ = encoder.child.wait();
        }
    }
}

struct BundledEncoder {
    fps: u32,
    encoder: Option<openh264::encoder::Encoder>,
    width: u32,
    height: u32,
}

impl BundledEncoder {
    fn new(fps: u32) -> Result<Self, A2aLabError> {
        if fps == 0 {
            return Err(A2aLabError::invalid("fps", "must be positive"));
        }
        Ok(Self {
            fps,
            encoder: None,
            width: 0,
            height: 0,
        })
    }

    fn push(&mut self, jpeg: &[u8]) -> Result<AccessUnit, A2aLabError> {
        let rgb = image::load_from_memory(jpeg)
            .map_err(|error| A2aLabError::invalid("jpeg", error.to_string()))?
            .to_rgb8();
        let width = rgb.width();
        let height = rgb.height();
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(A2aLabError::invalid(
                "jpeg",
                "H.264 input width and height must be positive even numbers",
            ));
        }
        if self.encoder.is_none() || self.width != width || self.height != height {
            self.encoder = Some(open_bundled_encoder(self.fps)?);
            self.width = width;
            self.height = height;
        }
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| A2aLabError::unavailable("OpenH264 encoder is closed"))?;
        let yuv = rgb_to_i420(rgb.as_raw(), width, height)?;
        encoder.force_intra_frame();
        let bitstream = encoder
            .encode(&yuv)
            .map_err(|error| A2aLabError::unavailable(format!("OpenH264 encode: {error}")))?;
        access_unit_from_annex(&bitstream.to_vec())
    }
}

fn save_bitrate_bps(fps: u32) -> u32 {
    // 2 Mbps at 30 fps. Storage tracks the save rate.
    2_000_000u32.saturating_mul(fps.max(1)) / 30
}

fn open_bundled_encoder(fps: u32) -> Result<openh264::encoder::Encoder, A2aLabError> {
    use openh264::encoder::{
        BitRate, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode,
    };
    let config = EncoderConfig::new()
        .skip_frames(false)
        .bitrate(BitRate::from_bps(save_bitrate_bps(fps)))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        .profile(Profile::Baseline)
        .intra_frame_period(IntraFramePeriod::from_num_frames(1))
        .num_threads(1);
    openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
        .map_err(|error| A2aLabError::unavailable(format!("start OpenH264 encoder: {error}")))
}

fn rgb_to_i420(
    rgb: &[u8],
    width: u32,
    height: u32,
) -> Result<openh264::formats::YUVBuffer, A2aLabError> {
    let mut planar =
        yuv::YuvPlanarImageMut::alloc(width, height, yuv::YuvChromaSubsampling::Yuv420);
    yuv::rgb_to_yuv420(
        &mut planar,
        rgb,
        width.saturating_mul(3),
        yuv::YuvRange::Limited,
        yuv::YuvStandardMatrix::Bt601,
        yuv::YuvConversionMode::Balanced,
    )
    .map_err(|error| A2aLabError::invalid("yuv", format!("{error:?}")))?;
    let mut packed = tight_plane(planar.y_plane.borrow(), planar.y_stride, width, height);
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    packed.extend(tight_plane(
        planar.u_plane.borrow(),
        planar.u_stride,
        chroma_width,
        chroma_height,
    ));
    packed.extend(tight_plane(
        planar.v_plane.borrow(),
        planar.v_stride,
        chroma_width,
        chroma_height,
    ));
    Ok(openh264::formats::YUVBuffer::from_vec(
        packed,
        usize::try_from(width).unwrap_or(0),
        usize::try_from(height).unwrap_or(0),
    ))
}

fn tight_plane(plane: &[u8], stride: u32, width: u32, height: u32) -> Vec<u8> {
    let stride = stride as usize;
    let width = width as usize;
    let mut out = Vec::with_capacity(width * height as usize);
    for row in 0..height as usize {
        let start = row * stride;
        out.extend_from_slice(&plane[start..start + width]);
    }
    out
}

fn access_unit_from_annex(bytes: &[u8]) -> Result<AccessUnit, A2aLabError> {
    let mut nals = Vec::new();
    let mut index = 0;
    let mut starts = Vec::new();
    while index + 3 < bytes.len() {
        if let Some(length) = start_code_len(&bytes[index..]) {
            starts.push((index, length));
            index += length;
        } else {
            index += 1;
        }
    }
    if starts.is_empty() {
        return Err(A2aLabError::invalid(
            "bitstream",
            "OpenH264 returned no NAL units",
        ));
    }
    for pair in starts.windows(2) {
        let (at, code) = pair[0];
        let (next, _) = pair[1];
        let nal = &bytes[at + code..next];
        if !nal.is_empty() {
            nals.push(nal.to_vec());
        }
    }
    let (tail, code) = starts[starts.len() - 1];
    let nal = &bytes[tail + code..];
    if !nal.is_empty() {
        nals.push(nal.to_vec());
    }
    let unit = access_unit(&nals);
    if unit.sample.is_empty() || unit.parameter_sets.is_empty() {
        return Err(A2aLabError::invalid(
            "bitstream",
            "OpenH264 access unit is missing parameter sets or a slice",
        ));
    }
    Ok(unit)
}

unsafe extern "C" {
    safe fn close(fd: i32) -> i32;
}

fn encoder_io(error: &std::io::Error) -> A2aLabError {
    A2aLabError::unavailable(format!("ffmpeg h264 encoder: {error}"))
}

pub(crate) fn drain_access_units(buffer: &mut Vec<u8>) -> Vec<AccessUnit> {
    let (nals, rest) = complete_nals(buffer);
    let mut units = Vec::new();
    let mut current = Vec::new();
    let mut seen_vcl = false;
    for nal in nals {
        let kind = nal_type(&nal);
        let vcl = kind.is_some_and(|kind| kind == 1 || kind == 5);
        // Another slice of this picture stays in the current unit. The next picture
        // starts at an access-unit delimiter or a new SPS.
        let picture_boundary = seen_vcl && matches!(kind, Some(7 | 9));
        if picture_boundary {
            units.push(access_unit(&current));
            current.clear();
            seen_vcl = false;
        }
        if vcl {
            seen_vcl = true;
        }
        current.push(nal);
    }
    let mut rebuilt = Vec::new();
    for nal in current {
        rebuilt.extend_from_slice(&[0, 0, 0, 1]);
        rebuilt.extend_from_slice(&nal);
    }
    rebuilt.extend_from_slice(rest);
    *buffer = rebuilt;
    units
}

fn access_unit(nals: &[Vec<u8>]) -> AccessUnit {
    let keyframe = nals.iter().any(|nal| nal_type(nal) == Some(5));
    let mut parameter_sets = Vec::new();
    let mut sample = Vec::new();
    for nal in nals {
        let kind = nal_type(nal);
        if matches!(kind, Some(7 | 8)) {
            parameter_sets.extend_from_slice(&[0, 0, 0, 1]);
            parameter_sets.extend_from_slice(nal);
        }
        // MSE decodes the slice from the media sample. Parameter sets stay in the init segment.
        if matches!(kind, Some(1 | 5)) {
            let length = u32::try_from(nal.len()).unwrap_or(u32::MAX);
            sample.extend_from_slice(&length.to_be_bytes());
            sample.extend_from_slice(nal);
        }
    }
    AccessUnit {
        keyframe,
        parameter_sets,
        sample,
    }
}

fn complete_nals(buffer: &[u8]) -> (Vec<Vec<u8>>, &[u8]) {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 < buffer.len() {
        if let Some(length) = start_code_len(&buffer[index..]) {
            starts.push((index, length));
            index += length;
        } else {
            index += 1;
        }
    }
    if starts.len() < 2 {
        return (Vec::new(), buffer);
    }
    let mut nals = Vec::new();
    for pair in starts.windows(2) {
        let (at, code) = pair[0];
        let (next, _) = pair[1];
        let nal = &buffer[at + code..next];
        if !nal.is_empty() {
            nals.push(nal.to_vec());
        }
    }
    let (tail, _) = starts[starts.len() - 1];
    (nals, &buffer[tail..])
}

fn start_code_len(bytes: &[u8]) -> Option<usize> {
    if bytes.starts_with(&[0, 0, 0, 1]) {
        Some(4)
    } else if bytes.starts_with(&[0, 0, 1]) {
        Some(3)
    } else {
        None
    }
}

fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|header| header & 0x1F)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn annex(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn a_closed_idr_picture_is_one_length_prefixed_sample() {
        let sps = [0x67_u8, 0x42];
        let pps = [0x68_u8, 0xCE];
        let idr_a = [0x65_u8, 0x11];
        let idr_b = [0x65_u8, 0x22];
        let mut buffer = annex(&[&sps, &pps, &idr_a, &sps, &pps, &idr_b]);
        buffer.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        let units = drain_access_units(&mut buffer);
        assert_eq!(units.len(), 1);
        assert!(units[0].keyframe);
        assert!(units[0].parameter_sets.starts_with(&[0, 0, 0, 1, 0x67]));
        assert!(units[0].sample.windows(2).any(|nal| nal == [0x65, 0x11]));
    }

    #[test]
    fn slices_of_one_picture_stay_in_one_sample() {
        let sps = [0x67_u8, 0x42];
        let pps = [0x68_u8, 0xCE];
        let slice_a = [0x65_u8, 0x11];
        let slice_b = [0x65_u8, 0x22];
        let next = [0x65_u8, 0x33];
        let mut buffer = annex(&[&sps, &pps, &slice_a, &slice_b, &sps, &pps, &next]);
        let units = drain_access_units(&mut buffer);
        assert_eq!(units.len(), 1);
        assert!(units[0].sample.windows(2).any(|nal| nal == [0x65, 0x11]));
        assert!(units[0].sample.windows(2).any(|nal| nal == [0x65, 0x22]));
        assert!(!units[0].sample.windows(2).any(|nal| nal == [0x65, 0x33]));
    }

    #[test]
    fn bundled_encoder_emits_an_idr_without_ffmpeg() {
        let mut rgb = vec![0_u8; 32 * 32 * 3];
        for pixel in rgb.chunks_exact_mut(3) {
            pixel[0] = 40;
            pixel[1] = 80;
            pixel[2] = 160;
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .write_image(&rgb, 32, 32, image::ExtendedColorType::Rgb8)
            .expect("jpeg");
        let unit = BundledEncoder::new(5).unwrap().push(&jpeg).expect("encode");
        assert!(unit.keyframe);
        assert!(!unit.parameter_sets.is_empty());
        assert!(!unit.sample.is_empty());
    }
}
