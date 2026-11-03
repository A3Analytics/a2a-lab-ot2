//! SQLite history for synchronized frame sets.

use std::path::Path;
use std::str::FromStr;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use super::error::FrameSetError;
use super::id::{FrameChannelId, FrameSetId, FrameSetSourceId};
use super::model::{
    ChannelProfile, ChannelRole, DepthScale, FrameChannel, FrameChannelParts, FrameSet,
    FrameSetMetadata, FrameSetSource, frame_channel,
};
use super::payload::{jpeg_ok, png_info};

const SCHEMA: &str = include_str!("../../migrations/003_frame_sets.sql");
const VERSION: i64 = 1;

/// Default SQLite file for frame sets.
pub const DEFAULT_FRAME_SET_DB: &str = ".a2a-lab-ot2/frame-sets.sqlite";

/// Persisted frame sets.
pub struct FrameSetHistory {
    pool: SqlitePool,
    max_channel_bytes: u64,
    #[cfg(test)]
    fail_next_write: AtomicBool,
}

impl FrameSetHistory {
    /// Opens or creates history and rejects incomplete rows.
    pub async fn open(path: &Path, max_channel_bytes: u64) -> Result<Self, FrameSetError> {
        if path.as_os_str().is_empty() || max_channel_bytes == 0 {
            return Err(FrameSetError::invalid(
                "frame_set_db",
                "path and payload limit must be present",
            ));
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                FrameSetError::Unavailable(format!("create {}: {error}", parent.display()))
            })?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(db_error)?;
        apply_schema(&pool).await?;
        let history = Self {
            pool,
            max_channel_bytes,
            #[cfg(test)]
            fail_next_write: AtomicBool::new(false),
        };
        history.verify().await?;
        Ok(history)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_write(&self) {
        self.fail_next_write.store(true, Ordering::SeqCst);
    }

    /// Inserts one complete set and evicts older sets for that source.
    pub async fn insert(
        &self,
        set: &FrameSet,
        retention: usize,
    ) -> Result<Vec<String>, FrameSetError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        insert_set(&mut tx, set).await?;
        let removed = prune(&mut tx, set.source_id().as_str(), retention).await?;
        #[cfg(test)]
        if self.fail_next_write.swap(false, Ordering::SeqCst) {
            return Err(FrameSetError::Unavailable(
                "frame-set history write failed".to_owned(),
            ));
        }
        tx.commit().await.map_err(db_error)?;
        Ok(removed)
    }

    /// Deletes one set. Channel rows cascade.
    pub async fn remove(&self, id: &FrameSetId) -> Result<(), FrameSetError> {
        sqlx::query("DELETE FROM frame_sets WHERE id = ?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// Allocates the next frame-set identifier.
    pub async fn next_id(&self) -> Result<FrameSetId, FrameSetError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let next = next_counter(&mut tx).await?;
        tx.commit().await.map_err(db_error)?;
        FrameSetId::new(format!("frame-{next}"))
    }

    /// Metadata page. This does not read payload bytes.
    pub async fn list(
        &self,
        source_id: Option<&FrameSetSourceId>,
        query: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<FramePage<FrameSetMetadata>, FrameSetError> {
        if limit == 0 {
            return Err(FrameSetError::invalid("limit", "must be positive"));
        }
        let offset = cursor_offset(cursor)?;
        let source = source_id.map_or("", FrameSetSourceId::as_str);
        let rows = sqlx::query(
            "SELECT id FROM frame_sets
             WHERE (?1 = '' OR source_id = ?1)
               AND (?2 = '' OR instr(id, ?2) > 0 OR instr(source_id, ?2) > 0
                    OR instr(attributes, ?2) > 0
                    OR EXISTS (
                        SELECT 1 FROM frame_channels
                        WHERE frame_set_id = frame_sets.id
                          AND (instr(role, ?2) > 0 OR instr(channel_id, ?2) > 0
                               OR instr(attributes, ?2) > 0)
                    ))
             ORDER BY captured_at_us, id
             LIMIT ?3 OFFSET ?4",
        )
        .bind(source)
        .bind(query)
        .bind(i64::from(limit) + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut items = Vec::new();
        for row in rows
            .iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
        {
            let id: String = row.try_get("id").map_err(db_error)?;
            items.push(self.metadata(&FrameSetId::new(id)?).await?);
        }
        let next = (rows.len() > usize::try_from(limit).unwrap_or(usize::MAX))
            .then(|| (offset + i64::from(limit)).to_string());
        Ok(FramePage {
            items,
            next_cursor: next,
        })
    }

    /// One stored set, including payload bytes.
    pub async fn get(&self, id: &FrameSetId) -> Result<FrameSet, FrameSetError> {
        self.load(id).await
    }

    /// One channel payload.
    pub async fn channel(
        &self,
        id: &FrameSetId,
        channel_id: &FrameChannelId,
    ) -> Result<FrameChannel, FrameSetError> {
        let set = self.load(id).await?;
        set.channel(channel_id)
            .cloned()
            .ok_or_else(|| FrameSetError::NotFound {
                kind: "frame channel",
                id: channel_id.to_string(),
            })
    }

    async fn metadata(&self, id: &FrameSetId) -> Result<FrameSetMetadata, FrameSetError> {
        Ok(self.load(id).await?.metadata())
    }

    async fn load(&self, id: &FrameSetId) -> Result<FrameSet, FrameSetError> {
        let row =
            sqlx::query("SELECT source_id, captured_at, attributes FROM frame_sets WHERE id = ?")
                .bind(id.as_str())
                .fetch_optional(&self.pool)
                .await
                .map_err(db_error)?
                .ok_or_else(|| FrameSetError::NotFound {
                    kind: "frame set",
                    id: id.to_string(),
                })?;
        let channels = load_channels(&self.pool, id).await?;
        if channels.is_empty() {
            return Err(FrameSetError::Corrupt(format!(
                "{id} has no channel payloads"
            )));
        }
        assemble(id, &row, channels, self.max_channel_bytes)
    }

    async fn verify(&self) -> Result<(), FrameSetError> {
        let ids = sqlx::query_scalar::<_, String>("SELECT id FROM frame_sets ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;
        for id in ids {
            let frame_id = FrameSetId::new(&id).map_err(|error| {
                FrameSetError::Corrupt(format!("{id} has an invalid identifier: {error}"))
            })?;
            self.load(&frame_id).await.map_err(|error| match error {
                FrameSetError::Corrupt(detail) => FrameSetError::Corrupt(detail),
                other => FrameSetError::Corrupt(format!("{id}: {other}")),
            })?;
        }
        Ok(())
    }
}

/// One page of frame-set metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct FramePage<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

impl<T> FramePage<T> {
    /// Page items.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Cursor for the next page.
    #[must_use]
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }
}

async fn apply_schema(pool: &SqlitePool) -> Result<(), FrameSetError> {
    if table_exists(pool, "frame_sets").await?
        && !column_exists(pool, "frame_sets", "captured_at_us").await?
    {
        return Err(FrameSetError::Corrupt(
            "frame-set history schema is incompatible; remove the database or migrate it before serving"
                .to_owned(),
        ));
    }
    for statement in SCHEMA.split(';') {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(db_error)?;
    }
    let versions = sqlx::query_scalar::<_, i64>("SELECT version FROM frame_set_schema")
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
    match versions.as_slice() {
        [] => {
            sqlx::query("INSERT INTO frame_set_schema (version) VALUES (?)")
                .bind(VERSION)
                .execute(pool)
                .await
                .map_err(db_error)?;
            sqlx::query("INSERT INTO frame_set_counter (next_id) VALUES (1)")
                .execute(pool)
                .await
                .map_err(db_error)?;
        }
        [VERSION] => {}
        _ => {
            return Err(FrameSetError::Corrupt(format!(
                "frame-set history schema version {versions:?} is not supported"
            )));
        }
    }
    Ok(())
}

async fn table_exists(pool: &SqlitePool, name: &str) -> Result<bool, FrameSetError> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    Ok(count > 0)
}

async fn column_exists(
    pool: &SqlitePool,
    table: &str,
    column: &str,
) -> Result<bool, FrameSetError> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .map_err(db_error)?;
    Ok(rows.iter().any(|row| {
        row.try_get::<String, _>("name")
            .is_ok_and(|name| name == column)
    }))
}

async fn insert_set(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    set: &FrameSet,
) -> Result<(), FrameSetError> {
    sqlx::query(
        "INSERT INTO frame_sets (id, source_id, captured_at, captured_at_us, attributes)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(set.id().as_str())
    .bind(set.source_id().as_str())
    .bind(set.captured_at())
    .bind(timestamp_us(set.captured_at())?)
    .bind(
        serde_json::to_string(set.attributes())
            .map_err(|error| FrameSetError::Malformed(format!("attributes: {error}")))?,
    )
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    for (position, channel) in set.channels().iter().enumerate() {
        insert_channel(
            tx,
            set.id(),
            i64::try_from(position).unwrap_or(i64::MAX),
            channel,
        )
        .await?;
    }
    Ok(())
}

async fn insert_channel(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &FrameSetId,
    position: i64,
    channel: &FrameChannel,
) -> Result<(), FrameSetError> {
    sqlx::query(
        "INSERT INTO frame_channels (
            frame_set_id, position, channel_id, role, media_type, width, height, bit_depth,
            device_timestamp_us, frame_number, depth_json, calibration_json, attributes, data
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.as_str())
    .bind(position)
    .bind(channel.id().as_str())
    .bind(channel.role().as_str())
    .bind(channel.media_type())
    .bind(i64::from(channel.width()))
    .bind(i64::from(channel.height()))
    .bind(i64::from(channel.bit_depth()))
    .bind(i64::try_from(channel.device_timestamp_us()).unwrap_or(i64::MAX))
    .bind(i64::try_from(channel.frame_number()).unwrap_or(i64::MAX))
    .bind(json_option(channel.depth())?)
    .bind(serde_json::to_string(channel.calibration()).map_err(json_error)?)
    .bind(serde_json::to_string(channel.attributes()).map_err(json_error)?)
    .bind(channel.data())
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

async fn prune(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    source_id: &str,
    retention: usize,
) -> Result<Vec<String>, FrameSetError> {
    let offset = i64::try_from(retention).unwrap_or(i64::MAX);
    let rows = sqlx::query(
        "SELECT id FROM frame_sets
         WHERE source_id = ?
         ORDER BY captured_at_us DESC, id DESC
         LIMIT -1 OFFSET ?",
    )
    .bind(source_id)
    .bind(offset)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_error)?;
    let mut removed = Vec::new();
    for row in rows {
        let id: String = row.try_get("id").map_err(db_error)?;
        sqlx::query("DELETE FROM frame_sets WHERE id = ?")
            .bind(&id)
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
        removed.push(id);
    }
    Ok(removed)
}

async fn next_counter(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<i64, FrameSetError> {
    let next = sqlx::query_scalar::<_, i64>("SELECT next_id FROM frame_set_counter")
        .fetch_one(&mut **tx)
        .await
        .map_err(db_error)?;
    sqlx::query("UPDATE frame_set_counter SET next_id = ?")
        .bind(next + 1)
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
    Ok(next)
}

async fn load_channels(
    pool: &SqlitePool,
    id: &FrameSetId,
) -> Result<Vec<FrameChannel>, FrameSetError> {
    let rows = sqlx::query(
        "SELECT channel_id, role, media_type, width, height, bit_depth, device_timestamp_us,
                frame_number, depth_json, calibration_json, attributes, data
         FROM frame_channels WHERE frame_set_id = ? ORDER BY position, channel_id",
    )
    .bind(id.as_str())
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    rows.iter().map(channel_from_row).collect()
}

fn channel_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<FrameChannel, FrameSetError> {
    let width = positive_u32(row.try_get("width").map_err(db_error)?, "width")?;
    let height = positive_u32(row.try_get("height").map_err(db_error)?, "height")?;
    let bit_depth = u8::try_from(row.try_get::<i64, _>("bit_depth").map_err(db_error)?)
        .map_err(|_| FrameSetError::Corrupt("bit depth is out of range".to_owned()))?;
    let data: Vec<u8> = row.try_get("data").map_err(db_error)?;
    let media_type: String = row.try_get("media_type").map_err(db_error)?;
    check_stored_payload(&media_type, width, height, bit_depth, &data)?;
    let role = ChannelRole::new(row.try_get::<String, _>("role").map_err(db_error)?)
        .map_err(|error| FrameSetError::Corrupt(error.to_string()))?;
    let depth = optional_json::<DepthScale>(row.try_get("depth_json").map_err(db_error)?)?;
    if role.is_depth() != depth.is_some() {
        return Err(FrameSetError::Corrupt(
            "depth scale does not match the stored channel role".to_owned(),
        ));
    }
    Ok(frame_channel(FrameChannelParts {
        id: FrameChannelId::new(row.try_get::<String, _>("channel_id").map_err(db_error)?)
            .map_err(|error| FrameSetError::Corrupt(error.to_string()))?,
        role,
        media_type,
        width,
        height,
        bit_depth,
        device_timestamp_us: u64::try_from(
            row.try_get::<i64, _>("device_timestamp_us")
                .map_err(db_error)?,
        )
        .map_err(|_| FrameSetError::Corrupt("device timestamp is negative".to_owned()))?,
        frame_number: u64::try_from(row.try_get::<i64, _>("frame_number").map_err(db_error)?)
            .map_err(|_| FrameSetError::Corrupt("frame number is negative".to_owned()))?,
        depth,
        calibration: required_json(&text_column(row, "calibration_json")?)?,
        attributes: required_json(&text_column(row, "attributes")?)?,
        data,
    }))
}

fn assemble(
    id: &FrameSetId,
    row: &sqlx::sqlite::SqliteRow,
    channels: Vec<FrameChannel>,
    max_channel_bytes: u64,
) -> Result<FrameSet, FrameSetError> {
    let profiles = channels
        .iter()
        .map(|channel| {
            ChannelProfile::new(
                channel.id().clone(),
                channel.role().clone(),
                channel.media_type(),
                channel.width(),
                channel.height(),
                channel.bit_depth(),
                1.0,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| FrameSetError::Corrupt(error.to_string()))?;
    let source_id = FrameSetSourceId::new(row.try_get::<String, _>("source_id").map_err(db_error)?)
        .map_err(|error| FrameSetError::Corrupt(error.to_string()))?;
    let source = FrameSetSource::new(
        source_id,
        "stored",
        "stored",
        "stored",
        profiles,
        "stored",
        u64::MAX,
    )
    .map_err(|error| FrameSetError::Corrupt(error.to_string()))?;
    let attributes: Map<String, Value> = required_json(&text_column(row, "attributes")?)?;
    FrameSet::new(
        id.clone(),
        &source,
        row.try_get::<String, _>("captured_at").map_err(db_error)?,
        channels,
        &[],
        attributes,
        max_channel_bytes,
    )
    .map_err(|error| FrameSetError::Corrupt(error.to_string()))
}

fn check_stored_payload(
    media_type: &str,
    width: u32,
    height: u32,
    bit_depth: u8,
    data: &[u8],
) -> Result<(), FrameSetError> {
    if data.is_empty() {
        return Err(FrameSetError::Corrupt(
            "stored channel payload is empty".to_owned(),
        ));
    }
    if media_type == "image/jpeg" {
        return if jpeg_ok(data) {
            Ok(())
        } else {
            Err(FrameSetError::Corrupt(
                "stored color payload is not a JPEG".to_owned(),
            ))
        };
    }
    let png = png_info(data).map_err(|error| FrameSetError::Corrupt(error.to_string()))?;
    if png.width == width
        && png.height == height
        && png.bit_depth == bit_depth
        && png.color_type == 0
    {
        Ok(())
    } else {
        Err(FrameSetError::Corrupt(
            "stored PNG header does not match its channel record".to_owned(),
        ))
    }
}

fn text_column(row: &sqlx::sqlite::SqliteRow, name: &str) -> Result<String, FrameSetError> {
    row.try_get(name).map_err(db_error)
}

fn positive_u32(value: i64, field: &str) -> Result<u32, FrameSetError> {
    u32::try_from(value)
        .map_err(|_| FrameSetError::Corrupt(format!("{field} is invalid")))
        .and_then(|value| {
            if value == 0 {
                Err(FrameSetError::Corrupt(format!("{field} is zero")))
            } else {
                Ok(value)
            }
        })
}

fn json_option<T: serde::Serialize>(value: Option<&T>) -> Result<Option<String>, FrameSetError> {
    value
        .map(|value| serde_json::to_string(value).map_err(json_error))
        .transpose()
}

fn required_json<T: serde::de::DeserializeOwned>(
    value: impl AsRef<str>,
) -> Result<T, FrameSetError> {
    serde_json::from_str(value.as_ref()).map_err(|error| FrameSetError::Corrupt(error.to_string()))
}

fn optional_json<T: serde::de::DeserializeOwned>(
    value: Option<String>,
) -> Result<Option<T>, FrameSetError> {
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| FrameSetError::Corrupt(error.to_string()))
        })
        .transpose()
}

fn timestamp_us(value: &str) -> Result<i64, FrameSetError> {
    let timestamp = jiff::Timestamp::from_str(value)
        .map_err(|_| FrameSetError::invalid("captured_at", "must be an RFC 3339 UTC timestamp"))?;
    Ok(timestamp.as_microsecond())
}

fn cursor_offset(cursor: Option<&str>) -> Result<i64, FrameSetError> {
    match cursor {
        None => Ok(0),
        Some(cursor) => cursor
            .parse()
            .map_err(|_| FrameSetError::invalid("cursor", "is not a valid page cursor")),
    }
}

fn db_error(error: impl std::fmt::Display) -> FrameSetError {
    FrameSetError::Unavailable(format!("frame-set history: {error}"))
}

fn json_error(error: impl std::fmt::Display) -> FrameSetError {
    FrameSetError::Malformed(error.to_string())
}
