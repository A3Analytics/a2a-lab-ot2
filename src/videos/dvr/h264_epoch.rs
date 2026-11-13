//! Legacy flat H.264 storage cutover onto devkit epoch tables.

use std::path::Path;

use a2a_lab_dev_kit::{A2aLabError, DvrTrackId, UtcTimestamp, VideoSourceId};
use sqlx::SqlitePool;
use tokio::task;

use super::store::{IndexedVideoDvr, db_error};

const CUT_OVER_KEY: &str = "h264_epoch_storage_cutover";

pub(crate) async fn migrate_h264_epochs(store: &IndexedVideoDvr) -> Result<(), A2aLabError> {
    if cutover_complete(store).await? {
        return Ok(());
    }
    scoped_h264_cutover_delete(store).await?;
    mark_cutover_complete(store).await
}

async fn cutover_complete(store: &IndexedVideoDvr) -> Result<bool, A2aLabError> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT meta_value FROM dvr_store_meta WHERE meta_key = ?1")
            .bind(CUT_OVER_KEY)
            .fetch_optional(store.sqlite_pool())
            .await
            .map_err(db_error)?;
    Ok(value.as_deref() == Some("1"))
}

async fn mark_cutover_complete(store: &IndexedVideoDvr) -> Result<(), A2aLabError> {
    sqlx::query(
        "INSERT INTO dvr_store_meta (meta_key, meta_value) VALUES (?1, '1')
         ON CONFLICT(meta_key) DO UPDATE SET meta_value = excluded.meta_value",
    )
    .bind(CUT_OVER_KEY)
    .execute(store.sqlite_pool())
    .await
    .map_err(db_error)?;
    Ok(())
}

async fn scoped_h264_cutover_delete(store: &IndexedVideoDvr) -> Result<(), A2aLabError> {
    let chunks: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT chunk_relpath FROM dvr_frames
         WHERE track_id = 'h264' AND chunk_relpath != ''",
    )
    .fetch_all(store.sqlite_pool())
    .await
    .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_frames WHERE track_id = 'h264'")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_chunks WHERE track_id = 'h264'")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    sqlx::query(
        "DELETE FROM dvr_segments
         WHERE track_id = 'h264'
            OR (source_id, track_id) IN (
                SELECT source_id, track_id FROM dvr_tracks WHERE track_id = 'h264'
            )",
    )
    .execute(store.sqlite_pool())
    .await
    .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_tracks WHERE track_id = 'h264'")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    sqlx::query("DELETE FROM dvr_h264_epochs")
        .execute(store.sqlite_pool())
        .await
        .map_err(db_error)?;
    remove_chunk_paths(store.root(), &chunks).await?;
    remove_h264_payload_dirs(store.root()).await
}

async fn remove_chunk_paths(root: &Path, chunks: &[String]) -> Result<(), A2aLabError> {
    let root = root.to_path_buf();
    let paths: Vec<std::path::PathBuf> = chunks
        .iter()
        .flat_map(|relpath| [root.join(relpath), root.join(format!("{relpath}.tmp"))])
        .collect();
    task::spawn_blocking(move || {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    })
    .await
    .map_err(super::store::join_error)?;
    Ok(())
}

async fn remove_h264_payload_dirs(root: &Path) -> Result<(), A2aLabError> {
    let root = root.to_path_buf();
    task::spawn_blocking(move || {
        if !root.is_dir() {
            return Ok::<_, A2aLabError>(());
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let source_dir = entry.path().join("h264");
            if source_dir.is_dir() {
                let _ = std::fs::remove_dir_all(source_dir);
            }
        }
        Ok(())
    })
    .await
    .map_err(super::store::join_error)??;
    Ok(())
}

pub(crate) async fn codec_init_for_frame(
    pool: &SqlitePool,
    source_id: &VideoSourceId,
    track_id: &DvrTrackId,
    captured_at: UtcTimestamp,
) -> Result<Option<Vec<u8>>, A2aLabError> {
    if track_id.as_str() != "h264" {
        return Ok(None);
    }
    let init: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT e.codec_init FROM dvr_frames f
         JOIN dvr_h264_epochs e ON e.epoch_id = f.h264_epoch_id
         WHERE f.source_id = ?1 AND f.track_id = ?2 AND f.start_us = ?3 AND f.payload_ready = 1",
    )
    .bind(source_id.as_str())
    .bind(track_id.as_str())
    .bind(captured_at.as_micros())
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    Ok(init)
}
