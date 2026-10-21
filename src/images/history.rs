//! SQLite history for requested camera frames.

use std::path::Path;
use std::str::FromStr;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, Image, ImageDescriptor, ImageId, ImageSourceId, ImageTransportConfig, JsonObject,
    UtcTimestamp,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::time::shift_seconds;

const SCHEMA: &str = include_str!("../../migrations/002_external_camera_images.sql");

/// Requested camera frames older than this are deleted.
pub const IMAGE_HISTORY_MAX_AGE: Duration = Duration::from_hours(7 * 24);

/// Default SQLite file for requested camera frames.
pub const DEFAULT_IMAGE_HISTORY_DB: &str = ".a2a-lab-ot2/images.sqlite";

/// Hourly age cleanup while serve is running.
pub const IMAGE_HISTORY_CLEANUP_INTERVAL: Duration = Duration::from_hours(1);

struct RestoredHistory {
    images: Vec<Image>,
    next_sequence: u64,
}

pub(crate) struct ImageHistory {
    pool: SqlitePool,
    source_ids: Vec<String>,
    #[cfg(test)]
    fail_next_write: AtomicBool,
}

struct ImageRow {
    id: String,
    source_id: String,
    captured_at: String,
    media_type: String,
    width: i64,
    height: i64,
    caption: Option<String>,
    attributes: String,
    data: Vec<u8>,
}

impl ImageHistory {
    pub(crate) async fn open(
        path: &Path,
        source_ids: &[ImageSourceId],
    ) -> Result<Self, A2aLabError> {
        if path.as_os_str().is_empty() {
            return Err(A2aLabError::invalid(
                "image_history_db",
                "must not be empty",
            ));
        }
        if source_ids.is_empty() {
            return Err(A2aLabError::invalid(
                "image history",
                "requires at least one source",
            ));
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                A2aLabError::unavailable(format!("create {}: {error}", parent.display()))
            })?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(db_error)?;
        for statement in SCHEMA.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&pool)
                .await
                .map_err(db_error)?;
        }
        migrate_external_history(&pool).await?;
        Ok(Self {
            pool,
            source_ids: source_ids
                .iter()
                .map(|source_id| source_id.as_str().to_owned())
                .collect(),
            #[cfg(test)]
            fail_next_write: AtomicBool::new(false),
        })
    }

    #[cfg(test)]
    pub(crate) fn fail_next_write(&self) {
        self.fail_next_write.store(true, Ordering::SeqCst);
    }

    pub(crate) async fn restore(
        &self,
        retention: usize,
        transport: ImageTransportConfig,
        now: UtcTimestamp,
    ) -> Result<(Vec<Image>, u64), A2aLabError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        prune(
            &mut tx,
            timestamp_us(history_cutoff(now)?)?,
            retention,
            None,
        )
        .await?;
        let rows = load_rows(&mut tx).await?;
        let restored = images_from_rows(rows, transport, &self.source_ids)?;
        tx.commit().await.map_err(db_error)?;
        Ok((restored.images, restored.next_sequence))
    }

    pub(crate) async fn record(
        &self,
        image: &Image,
        retention: usize,
        now: UtcTimestamp,
    ) -> Result<Vec<String>, A2aLabError> {
        #[cfg(test)]
        if self.fail_next_write.swap(false, Ordering::SeqCst) {
            return Err(A2aLabError::unavailable("image history write failed"));
        }
        if !self
            .source_ids
            .iter()
            .any(|source_id| source_id == image.descriptor().source_id().as_str())
        {
            return Err(A2aLabError::invalid(
                "image history",
                "frame is not from a persisted source",
            ));
        }
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        insert_image(&mut tx, image).await?;
        let removed = prune(
            &mut tx,
            timestamp_us(history_cutoff(now)?)?,
            retention,
            Some(image.descriptor().id().as_str()),
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(removed)
    }

    pub(crate) async fn expire(
        &self,
        retention: usize,
        now: UtcTimestamp,
    ) -> Result<Vec<String>, A2aLabError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let removed = prune(
            &mut tx,
            timestamp_us(history_cutoff(now)?)?,
            retention,
            None,
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(removed)
    }
}

async fn migrate_external_history(pool: &SqlitePool) -> Result<(), A2aLabError> {
    let legacy = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'external_camera_images'",
    )
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    if legacy == 0 {
        return Ok(());
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    sqlx::query(
        "INSERT OR IGNORE INTO image_history (
             id, source_id, captured_at, captured_at_us, media_type,
             width, height, caption, attributes, data
         )
         SELECT id, source_id, captured_at, captured_at_us, media_type,
                width, height, caption, attributes, data
         FROM external_camera_images",
    )
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    sqlx::query("DROP TABLE external_camera_images")
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    tx.commit().await.map_err(db_error)
}

fn history_cutoff(now: UtcTimestamp) -> Result<UtcTimestamp, A2aLabError> {
    let age = i64::try_from(IMAGE_HISTORY_MAX_AGE.as_secs())
        .map_err(|_| A2aLabError::invalid("image history", "age is out of range"))?;
    shift_seconds(now, -age)
}

fn timestamp_us(stamp: UtcTimestamp) -> Result<i64, A2aLabError> {
    let timestamp = jiff::Timestamp::from_str(&stamp.to_rfc3339())
        .map_err(|_| A2aLabError::invalid("timestamp", "unparseable"))?;
    Ok(timestamp.as_microsecond())
}

async fn prune(
    tx: &mut Transaction<'_, Sqlite>,
    cutoff_us: i64,
    retention: usize,
    keep: Option<&str>,
) -> Result<Vec<String>, A2aLabError> {
    let mut removed = delete_aged(tx, cutoff_us, keep).await?;
    removed.extend(delete_excess(tx, retention, keep).await?);
    Ok(removed)
}

async fn delete_aged(
    tx: &mut Transaction<'_, Sqlite>,
    cutoff_us: i64,
    keep: Option<&str>,
) -> Result<Vec<String>, A2aLabError> {
    let rows = if let Some(id) = keep {
        sqlx::query_as::<_, (String,)>(
            "DELETE FROM image_history
                 WHERE captured_at_us < ?1 AND id != ?2
                 RETURNING id",
        )
        .bind(cutoff_us)
        .bind(id)
        .fetch_all(&mut **tx)
        .await
        .map_err(db_error)?
    } else {
        sqlx::query_as::<_, (String,)>(
            "DELETE FROM image_history
                 WHERE captured_at_us < ?1
                 RETURNING id",
        )
        .bind(cutoff_us)
        .fetch_all(&mut **tx)
        .await
        .map_err(db_error)?
    };
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn delete_excess(
    tx: &mut Transaction<'_, Sqlite>,
    retention: usize,
    keep: Option<&str>,
) -> Result<Vec<String>, A2aLabError> {
    let rows = if let Some(id) = keep {
        let kept = i64::try_from(retention).unwrap_or(i64::MAX);
        sqlx::query_as::<_, (String,)>(
            "WITH ranked AS (
                 SELECT id, ROW_NUMBER() OVER (
                     PARTITION BY source_id
                     ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END,
                              captured_at_us DESC, id DESC
                 ) AS position
                 FROM image_history
             )
             DELETE FROM image_history
                 WHERE id IN (SELECT id FROM ranked WHERE position > ?2)
                 RETURNING id",
        )
        .bind(id)
        .bind(kept)
        .fetch_all(&mut **tx)
        .await
        .map_err(db_error)?
    } else {
        let kept = i64::try_from(retention).unwrap_or(i64::MAX);
        sqlx::query_as::<_, (String,)>(
            "WITH ranked AS (
                 SELECT id, ROW_NUMBER() OVER (
                     PARTITION BY source_id
                     ORDER BY captured_at_us DESC, id DESC
                 ) AS position
                 FROM image_history
             )
             DELETE FROM image_history
                 WHERE id IN (SELECT id FROM ranked WHERE position > ?1)
                 RETURNING id",
        )
        .bind(kept)
        .fetch_all(&mut **tx)
        .await
        .map_err(db_error)?
    };
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn insert_image(tx: &mut Transaction<'_, Sqlite>, image: &Image) -> Result<(), A2aLabError> {
    let descriptor = image.descriptor();
    let attributes = serde_json::to_string(descriptor.attributes())
        .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
    sqlx::query(
        "INSERT INTO image_history (
            id, source_id, captured_at, captured_at_us, media_type, width, height, caption, attributes, data
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(descriptor.id().as_str())
    .bind(descriptor.source_id().as_str())
    .bind(descriptor.captured_at().to_rfc3339())
    .bind(timestamp_us(descriptor.captured_at())?)
    .bind(descriptor.media_type())
    .bind(i64::from(descriptor.width()))
    .bind(i64::from(descriptor.height()))
    .bind(descriptor.caption())
    .bind(attributes)
    .bind(image.data())
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

async fn load_rows(tx: &mut Transaction<'_, Sqlite>) -> Result<Vec<ImageRow>, A2aLabError> {
    let rows = sqlx::query(
        "SELECT id, source_id, captured_at, media_type, width, height, caption, attributes, data
         FROM image_history
         ORDER BY captured_at_us, id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(db_error)?;
    rows.iter().map(image_row).collect()
}

fn image_row(row: &SqliteRow) -> Result<ImageRow, A2aLabError> {
    Ok(ImageRow {
        id: row.try_get("id").map_err(db_error)?,
        source_id: row.try_get("source_id").map_err(db_error)?,
        captured_at: row.try_get("captured_at").map_err(db_error)?,
        media_type: row.try_get("media_type").map_err(db_error)?,
        width: row.try_get("width").map_err(db_error)?,
        height: row.try_get("height").map_err(db_error)?,
        caption: row.try_get("caption").map_err(db_error)?,
        attributes: row.try_get("attributes").map_err(db_error)?,
        data: row.try_get("data").map_err(db_error)?,
    })
}

fn images_from_rows(
    rows: Vec<ImageRow>,
    transport: ImageTransportConfig,
    source_ids: &[String],
) -> Result<RestoredHistory, A2aLabError> {
    let mut next_sequence = 0;
    let mut images = Vec::with_capacity(rows.len());
    for row in rows {
        next_sequence = next_sequence.max(sequence(&row.id)?);
        if !source_ids
            .iter()
            .any(|source_id| source_id == &row.source_id)
        {
            continue;
        }
        images.push(image_from_row(row, transport, source_ids)?);
    }
    Ok(RestoredHistory {
        images,
        next_sequence,
    })
}

fn image_from_row(
    row: ImageRow,
    transport: ImageTransportConfig,
    source_ids: &[String],
) -> Result<Image, A2aLabError> {
    let id = row.id;
    if !source_ids
        .iter()
        .any(|source_id| source_id == &row.source_id)
    {
        return Err(corrupt(&id, "source"));
    }
    let captured_at =
        UtcTimestamp::parse(&row.captured_at).map_err(|_| corrupt(&id, "timestamp"))?;
    let width = u32::try_from(row.width).map_err(|_| corrupt(&id, "width"))?;
    let height = u32::try_from(row.height).map_err(|_| corrupt(&id, "height"))?;
    let attributes = JsonObject::parse(&row.attributes).map_err(|_| corrupt(&id, "attributes"))?;
    let descriptor = ImageDescriptor::new(
        ImageId::new(&id).map_err(|_| corrupt(&id, "id"))?,
        ImageSourceId::new(row.source_id).map_err(|_| corrupt(&id, "source"))?,
        captured_at,
        row.media_type,
        width,
        height,
        row.caption,
        attributes,
    )
    .map_err(|error| history_row(&id, &error))?;
    Image::with_transport(descriptor, row.data, transport).map_err(|error| history_row(&id, &error))
}

fn sequence(id: &str) -> Result<u64, A2aLabError> {
    let Some(digits) = id.strip_prefix("img-") else {
        return Err(corrupt(id, "id"));
    };
    if digits.len() != 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(corrupt(id, "id"));
    }
    digits.parse().map_err(|_| corrupt(id, "id"))
}

fn corrupt(id: &str, field: &str) -> A2aLabError {
    A2aLabError::invalid("image history", format!("{id} has a corrupt {field}"))
}

fn history_row(id: &str, error: &A2aLabError) -> A2aLabError {
    A2aLabError::invalid("image history", format!("{id}: {error}"))
}

fn db_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::unavailable(format!("image history: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "a2a-lab-ot2-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn image_at(stamp: UtcTimestamp) -> Image {
        Image::with_transport(
            ImageDescriptor::new(
                ImageId::new("img-00000000000000000001").unwrap(),
                ImageSourceId::new("external-camera").unwrap(),
                stamp,
                "image/jpeg",
                1,
                1,
                None,
                JsonObject::empty(),
            )
            .unwrap(),
            vec![1, 2, 3],
            ImageTransportConfig::new(1024).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn exactly_seven_days_remains_and_one_more_second_expires() {
        let dir = scratch("age-boundary");
        let source = ImageSourceId::new("external-camera").unwrap();
        let history = ImageHistory::open(dir.join("images.sqlite").as_path(), &[source])
            .await
            .unwrap();
        let now = UtcTimestamp::parse("2026-10-20T00:00:00Z").unwrap();
        let cutoff = history_cutoff(now).unwrap();
        history.record(&image_at(cutoff), 4, now).await.unwrap();
        let (kept, next) = history
            .restore(4, ImageTransportConfig::new(1024).unwrap(), now)
            .await
            .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(next, 1);
        let (expired, _) = history
            .restore(
                4,
                ImageTransportConfig::new(1024).unwrap(),
                shift_seconds(now, 1).unwrap(),
            )
            .await
            .unwrap();
        assert!(expired.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
