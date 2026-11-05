//! Host free-space and path checks for synchronized history.

use std::path::Path;
use std::process::Command;

use crate::frame_sets::FrameSetError;

/// Default directory for retained synchronized payloads.
pub const DEFAULT_SYNC_CHUNK_ROOT: &str = ".a2a-lab-ot2/sync-chunks";

/// Bytes still free on the volume that contains `root`.
pub fn free_bytes(root: &Path) -> Result<u64, String> {
    let path = if root.exists() { root } else { Path::new(".") };
    let output = Command::new("df")
        .args(["-k", "-P"])
        .arg(path)
        .output()
        .map_err(|error| format!("df {}: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!("df {}: failed", path.display()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .nth(1)
        .ok_or_else(|| "df: no rows".to_owned())?;
    let avail = line
        .split_whitespace()
        .nth(3)
        .ok_or_else(|| "df: missing available column".to_owned())?;
    let kib = avail
        .parse::<u64>()
        .map_err(|_| format!("df: unreadable available `{avail}`"))?;
    Ok(kib.saturating_mul(1024))
}

/// Rejects overlapping SQLite files or chunk roots before serve binds.
pub fn validate_sync_paths(
    sync_db: &Path,
    frame_set_db: &Path,
    chunk_root: &Path,
    video_dvr_root: &Path,
) -> Result<(), FrameSetError> {
    if sync_db == frame_set_db {
        return Err(FrameSetError::invalid(
            "sync_db",
            "must not be the same file as frame-set history",
        ));
    }
    if chunk_root == video_dvr_root {
        return Err(FrameSetError::invalid(
            "sync_chunk_root",
            "must not be the same directory as video DVR",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_sync_paths;
    use std::path::Path;

    #[test]
    fn overlapping_history_paths_are_rejected() {
        let db = Path::new("history.sqlite");
        let chunks = Path::new("chunks");
        let dvr = Path::new("dvr");
        assert!(validate_sync_paths(db, db, chunks, dvr).is_err());
        assert!(validate_sync_paths(db, Path::new("frames.sqlite"), dvr, dvr).is_err());
        assert!(validate_sync_paths(db, Path::new("frames.sqlite"), chunks, dvr).is_ok());
    }
}
