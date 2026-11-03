//! Append-only per-minute MJPEG chunk files with indexed frame offsets.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use a2a_lab_dev_kit::{A2aLabError, UtcTimestamp};

pub const MINUTE_US: i64 = 60_000_000;

#[must_use]
pub fn minute_start_us(at: UtcTimestamp) -> i64 {
    let micros = at.as_micros();
    micros - micros.rem_euclid(MINUTE_US)
}

#[must_use]
pub fn chunk_relpath(source_id: &str, track_id: &str, minute_start_us: i64) -> String {
    format!("{source_id}/{track_id}/{minute_start_us}.chunk")
}

#[must_use]
pub fn chunk_tmp_relpath(source_id: &str, track_id: &str, minute_start_us: i64) -> String {
    format!("{source_id}/{track_id}/{minute_start_us}.chunk.tmp")
}

#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

pub fn verify_crc32(bytes: &[u8], expected: u32) -> Result<(), A2aLabError> {
    let actual = crc32(bytes);
    if actual != expected {
        return Err(A2aLabError::unavailable(format!(
            "video DVR payload checksum mismatch: expected {expected:#010x}, got {actual:#010x}"
        )));
    }
    Ok(())
}

pub async fn ensure_parent(path: &Path) -> Result<(), A2aLabError> {
    if let Some(parent) = path.parent() {
        let parent = parent.to_path_buf();
        let display = parent.display().to_string();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&parent))
            .await
            .map_err(join_error)?
            .map_err(|error| A2aLabError::unavailable(format!("create {display}: {error}")))?;
    }
    Ok(())
}

pub async fn append_payload(tmp: &Path, payload: &[u8]) -> Result<(u64, u32), A2aLabError> {
    ensure_parent(tmp).await?;
    let tmp = tmp.to_path_buf();
    let payload = payload.to_vec();
    let checksum = crc32(&payload);
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&tmp)
            .map_err(|error| {
                A2aLabError::unavailable(format!("open {}: {error}", tmp.display()))
            })?;
        let offset = file.seek(SeekFrom::End(0)).map_err(|error| {
            A2aLabError::unavailable(format!("seek {}: {error}", tmp.display()))
        })?;
        file.write_all(&payload).map_err(|error| {
            A2aLabError::unavailable(format!("write {}: {error}", tmp.display()))
        })?;
        file.sync_all().map_err(|error| {
            A2aLabError::unavailable(format!("sync {}: {error}", tmp.display()))
        })?;
        Ok((offset, checksum))
    })
    .await
    .map_err(join_error)?
}

pub async fn finalize_chunk(tmp: &Path, final_path: &Path) -> Result<(), A2aLabError> {
    let tmp = tmp.to_path_buf();
    let final_path = final_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if !tmp.exists() {
            return Ok(());
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(&tmp)
            .map_err(|error| {
                A2aLabError::unavailable(format!("open {}: {error}", tmp.display()))
            })?;
        file.sync_all().map_err(|error| {
            A2aLabError::unavailable(format!("sync {}: {error}", tmp.display()))
        })?;
        if final_path.exists() {
            std::fs::remove_file(&tmp).map_err(|error| {
                A2aLabError::unavailable(format!("remove {}: {error}", tmp.display()))
            })?;
            return Ok(());
        }
        std::fs::rename(&tmp, &final_path).map_err(|error| {
            A2aLabError::unavailable(format!(
                "rename {} -> {}: {error}",
                tmp.display(),
                final_path.display()
            ))
        })
    })
    .await
    .map_err(join_error)?
}

pub async fn read_payload_range(
    path: &Path,
    offset: u64,
    length: u64,
    expected_crc32: u32,
) -> Result<Vec<u8>, A2aLabError> {
    let path = path.to_path_buf();
    let display = path.display().to_string();
    let bytes = tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)
            .map_err(|error| A2aLabError::unavailable(format!("open {display}: {error}")))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| A2aLabError::unavailable(format!("seek {display}: {error}")))?;
        let len = usize::try_from(length).unwrap_or(usize::MAX);
        let mut buf = vec![0_u8; len];
        file.read_exact(&mut buf)
            .map_err(|error| A2aLabError::unavailable(format!("read {display}: {error}")))?;
        Ok::<_, A2aLabError>(buf)
    })
    .await
    .map_err(join_error)??;
    verify_crc32(&bytes, expected_crc32)?;
    Ok(bytes)
}

pub fn truncate_chunk_files(root: &Path, truncate_to: &[(String, u64)]) -> Result<(), A2aLabError> {
    let mut limits: HashMap<String, u64> = HashMap::new();
    for (relpath, end) in truncate_to {
        limits
            .entry(relpath.clone())
            .and_modify(|current| *current = (*current).max(*end))
            .or_insert(*end);
    }
    walk_chunk_files(root, &mut |path, relpath| {
        let base = relpath.strip_suffix(".tmp").unwrap_or(relpath).to_string();
        let Some(limit) = limits.get(&base) else {
            return Ok(());
        };
        if *limit == 0 {
            return Ok(());
        }
        truncate_file(path, *limit)?;
        Ok(())
    })
}

fn truncate_file(path: &Path, len: u64) -> Result<(), A2aLabError> {
    let mut bytes = std::fs::read(path)
        .map_err(|error| A2aLabError::unavailable(format!("read {}: {error}", path.display())))?;
    let len = usize::try_from(len).unwrap_or(bytes.len());
    bytes.truncate(len);
    std::fs::write(path, bytes)
        .map_err(|error| A2aLabError::unavailable(format!("write {}: {error}", path.display())))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| A2aLabError::unavailable(format!("open {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| A2aLabError::unavailable(format!("sync {}: {error}", path.display())))
}

fn walk_chunk_files(
    root: &Path,
    handle: &mut dyn FnMut(&Path, &str) -> Result<(), A2aLabError>,
) -> Result<(), A2aLabError> {
    let root = root.canonicalize().map_err(|error| {
        A2aLabError::unavailable(format!("canonicalize {}: {error}", root.display()))
    })?;
    walk_chunk_files_inner(&root, &root, handle)
}

fn walk_chunk_files_inner(
    root: &Path,
    dir: &Path,
    handle: &mut dyn FnMut(&Path, &str) -> Result<(), A2aLabError>,
) -> Result<(), A2aLabError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|error| A2aLabError::unavailable(format!("read {}: {error}", dir.display())))?
    {
        let entry = entry.map_err(|error| {
            A2aLabError::unavailable(format!("read {}: {error}", dir.display()))
        })?;
        let path = entry.path();
        if path.is_dir() {
            walk_chunk_files_inner(root, &path, handle)?;
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.ends_with(".chunk.tmp")
            || Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("chunk"))
        {
            let relpath = path
                .strip_prefix(root)
                .map_err(|error| {
                    A2aLabError::unavailable(format!(
                        "strip {} from {}: {error}",
                        path.display(),
                        root.display()
                    ))
                })?
                .to_string_lossy()
                .into_owned();
            handle(&path, &relpath)?;
        }
    }
    Ok(())
}

fn join_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::unavailable(format!("video DVR task: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{crc32, minute_start_us};
    use a2a_lab_dev_kit::UtcTimestamp;

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn minute_floor_aligns_to_utc_minute() {
        let at = UtcTimestamp::parse("2024-06-01T00:00:59.500000Z").unwrap();
        assert_eq!(
            minute_start_us(at),
            UtcTimestamp::parse("2024-06-01T00:00:00Z")
                .unwrap()
                .as_micros()
        );
    }

    #[test]
    fn truncate_shortens_active_chunk_tmp() {
        use super::truncate_chunk_files;

        let dir =
            std::env::temp_dir().join(format!("a2a-lab-ot2-chunk-trunc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let relpath = "cam-a/track/123.chunk";
        let path = dir.join(format!("{relpath}.tmp"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![0_u8; 8]).unwrap();
        truncate_chunk_files(&dir, &[(relpath.to_string(), 4)]).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
