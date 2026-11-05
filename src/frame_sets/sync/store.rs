//! SQLite index of complete synchronized sets.

use std::path::{Path, PathBuf};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tokio::fs;

use super::disk::free_bytes;
use crate::frame_sets::FrameSetError;

const SCHEMA: &str = include_str!("../../../migrations/006_sync_history.sql");

/// Default SQLite file for synchronized history.
pub const DEFAULT_SYNC_DB: &str = ".a2a-lab-ot2/sync.sqlite";

/// 256 GiB payload quota.
pub const PAYLOAD_QUOTA: u64 = 256 * 1024 * 1024 * 1024;

/// Keep 64 GiB free on the chunk volume.
pub const FREE_RESERVE: u64 = 64 * 1024 * 1024 * 1024;

/// One stored channel payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncChannel {
    /// Portable channel id.
    pub channel_id: String,
    /// Encoded media type.
    pub media_type: String,
    /// Complete payload bytes.
    pub payload: Vec<u8>,
}

/// One complete synchronized set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSet {
    /// Stable set id.
    pub id: String,
    /// Portable source id.
    pub source_id: String,
    /// Device serial.
    pub serial: String,
    /// Host capture time.
    pub captured_at: String,
    /// Device timestamp in microseconds.
    pub timestamp_us: u64,
    /// True when depth and both infrared planes are present.
    pub stereo: bool,
    /// Channel payloads for the current mode.
    pub channels: Vec<SyncChannel>,
}

/// Restart-safe synchronized history.
#[derive(Clone)]
pub struct SyncStore {
    pool: SqlitePool,
    chunk_root: PathBuf,
    quota: u64,
    reserve: u64,
}

impl SyncStore {
    /// Opens or creates history and drops unfinished rows.
    pub async fn open(db: &Path, chunk_root: &Path) -> Result<Self, FrameSetError> {
        Self::open_with_limits(db, chunk_root, PAYLOAD_QUOTA, FREE_RESERVE).await
    }

    /// Opens history with explicit quota and free-space reserve.
    pub async fn open_with_limits(
        db: &Path,
        chunk_root: &Path,
        quota: u64,
        reserve: u64,
    ) -> Result<Self, FrameSetError> {
        if let Some(parent) = db.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).await.map_err(|error| {
                FrameSetError::Unavailable(format!("create {}: {error}", parent.display()))
            })?;
        }
        fs::create_dir_all(chunk_root).await.map_err(|error| {
            FrameSetError::Unavailable(format!("create {}: {error}", chunk_root.display()))
        })?;
        let options = SqliteConnectOptions::new()
            .filename(db)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(|error| {
                FrameSetError::Unavailable(format!("open {}: {error}", db.display()))
            })?;
        sqlx::query(SCHEMA)
            .execute(&pool)
            .await
            .map_err(|error| FrameSetError::Unavailable(format!("schema: {error}")))?;
        sqlx::query("DELETE FROM sync_channels WHERE payload_ready = 0")
            .execute(&pool)
            .await
            .map_err(|error| FrameSetError::Unavailable(format!("drop unfinished: {error}")))?;
        Ok(Self {
            pool,
            chunk_root: chunk_root.to_path_buf(),
            quota,
            reserve,
        })
    }

    /// Chunk directory used by this store.
    #[must_use]
    pub fn chunk_root(&self) -> &Path {
        &self.chunk_root
    }

    /// Inserts one complete set and evicts oldest minutes when over quota.
    pub async fn insert(&self, set: &SyncSet) -> Result<(), FrameSetError> {
        if set.channels.is_empty() {
            return Err(FrameSetError::invalid(
                "channels",
                "a sync set has no channels",
            ));
        }
        let minute = set.timestamp_us / 60_000_000 * 60_000_000;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| FrameSetError::Unavailable(format!("begin: {error}")))?;
        sqlx::query(
            "INSERT INTO sync_sets (id, source_id, serial, captured_at, timestamp_us, stereo, minute_start_us)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&set.id)
        .bind(&set.source_id)
        .bind(&set.serial)
        .bind(&set.captured_at)
        .bind(i64::try_from(set.timestamp_us).unwrap_or(i64::MAX))
        .bind(i64::from(set.stereo))
        .bind(i64::try_from(minute).unwrap_or(i64::MAX))
        .execute(&mut *tx)
        .await
        .map_err(|error| FrameSetError::Unavailable(format!("insert set: {error}")))?;
        for channel in &set.channels {
            sqlx::query(
                "INSERT INTO sync_channels (set_id, channel_id, media_type, payload, payload_length, payload_ready)
                 VALUES (?, ?, ?, ?, ?, 1)",
            )
            .bind(&set.id)
            .bind(&channel.channel_id)
            .bind(&channel.media_type)
            .bind(&channel.payload)
            .bind(i64::try_from(channel.payload.len()).unwrap_or(i64::MAX))
            .execute(&mut *tx)
            .await
            .map_err(|error| FrameSetError::Unavailable(format!("insert channel: {error}")))?;
        }
        tx.commit()
            .await
            .map_err(|error| FrameSetError::Unavailable(format!("commit: {error}")))?;
        self.evict().await
    }

    /// Color payload for one retained set.
    pub async fn color_payload(&self, id: &str) -> Result<Option<Vec<u8>>, FrameSetError> {
        let row = sqlx::query(
            "SELECT payload FROM sync_channels WHERE set_id = ? AND channel_id = 'color' AND payload_ready = 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| FrameSetError::Unavailable(format!("color: {error}")))?;
        Ok(row.map(|row| row.get("payload")))
    }

    /// Ready payload bytes across all channels.
    pub async fn payload_bytes(&self) -> Result<u64, FrameSetError> {
        let total: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(payload_length), 0) FROM sync_channels WHERE payload_ready = 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| FrameSetError::Unavailable(format!("sum: {error}")))?;
        u64::try_from(total)
            .map_err(|_| FrameSetError::Unavailable("payload length overflow".to_owned()))
    }

    /// Distinct retained set ids, oldest first.
    pub async fn set_ids(&self) -> Result<Vec<String>, FrameSetError> {
        sqlx::query_scalar("SELECT id FROM sync_sets ORDER BY timestamp_us, id")
            .fetch_all(&self.pool)
            .await
            .map_err(|error| FrameSetError::Unavailable(format!("ids: {error}")))
    }

    async fn evict(&self) -> Result<(), FrameSetError> {
        loop {
            let used = self.payload_bytes().await?;
            let free = free_bytes(&self.chunk_root).unwrap_or(u64::MAX);
            if used <= self.quota && free >= self.reserve {
                return Ok(());
            }
            let minute: Option<i64> =
                sqlx::query_scalar("SELECT MIN(minute_start_us) FROM sync_sets")
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|error| {
                        FrameSetError::Unavailable(format!("oldest minute: {error}"))
                    })?;
            let Some(minute) = minute else {
                return Ok(());
            };
            let deleted = sqlx::query("DELETE FROM sync_sets WHERE minute_start_us = ?")
                .bind(minute)
                .execute(&self.pool)
                .await
                .map_err(|error| FrameSetError::Unavailable(format!("evict: {error}")))?;
            if deleted.rows_affected() == 0 {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod store_tests {
    use super::{PAYLOAD_QUOTA, SyncChannel, SyncSet, SyncStore};

    fn jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xD9]
    }

    fn set(id: &str, timestamp_us: u64, channels: &[&str]) -> SyncSet {
        SyncSet {
            id: id.to_owned(),
            source_id: "shared-camera".to_owned(),
            serial: "ABC".to_owned(),
            captured_at: "2026-11-05T00:00:00Z".to_owned(),
            timestamp_us,
            stereo: channels.len() > 1,
            channels: channels
                .iter()
                .map(|channel| SyncChannel {
                    channel_id: (*channel).to_owned(),
                    media_type: "application/octet-stream".to_owned(),
                    payload: jpeg(),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn one_copy_color_and_dvr_index() {
        let root = std::env::temp_dir().join(format!("sync-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = SyncStore::open_with_limits(
            &root.join("index.sqlite"),
            &root.join("chunks"),
            PAYLOAD_QUOTA,
            0,
        )
        .await
        .unwrap();
        store
            .insert(&set("sync-1", 1_000, &["color", "depth"]))
            .await
            .unwrap();
        assert_eq!(store.color_payload("sync-1").await.unwrap(), Some(jpeg()));
        assert_eq!(store.set_ids().await.unwrap(), ["sync-1"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn eviction_drops_all_channels_in_oldest_minute() {
        let root = std::env::temp_dir().join(format!("sync-evict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store =
            SyncStore::open_with_limits(&root.join("index.sqlite"), &root.join("chunks"), 12, 0)
                .await
                .unwrap();
        store
            .insert(&set(
                "sync-old",
                1_000,
                &["color", "depth", "infrared-left"],
            ))
            .await
            .unwrap();
        store
            .insert(&set(
                "sync-new",
                60_000_000,
                &["color", "depth", "infrared-left"],
            ))
            .await
            .unwrap();
        assert_eq!(store.set_ids().await.unwrap(), ["sync-new"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
