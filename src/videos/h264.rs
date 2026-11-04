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

/// Long-lived encoder. One JPEG in, one access unit out, one frame behind.
pub(crate) struct H264Encoder {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<AccessUnit>,
    ready: std::collections::VecDeque<AccessUnit>,
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
    /// Starts `ffmpeg` reading MJPEG on stdin and writing Annex B on stdout.
    pub(crate) fn spawn() -> Result<Self, A2aLabError> {
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
        let mut child = command.spawn().map_err(|error| {
            A2aLabError::unavailable(format!("start ffmpeg h264 encoder: {error}"))
        })?;
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
        Ok(Self {
            child,
            stdin,
            incoming,
            ready: std::collections::VecDeque::new(),
        })
    }

    /// Writes one JPEG and returns the previous picture once the encoder has closed it.
    pub(crate) fn push(&mut self, jpeg: &[u8]) -> Result<Option<AccessUnit>, A2aLabError> {
        self.stdin
            .write_all(jpeg)
            .map_err(|error| encoder_io(&error))?;
        self.stdin.flush().map_err(|error| encoder_io(&error))?;
        self.drain_ready();
        if let Some(status) = self.child.try_wait().map_err(|error| encoder_io(&error))? {
            return Err(A2aLabError::unavailable(format!(
                "ffmpeg h264 encoder exited ({status})"
            )));
        }
        Ok(self.ready.pop_front())
    }

    fn drain_ready(&mut self) {
        while let Ok(unit) = self.incoming.try_recv() {
            self.ready.push_back(unit);
        }
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
}
