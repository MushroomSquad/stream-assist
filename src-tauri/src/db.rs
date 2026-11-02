use std::path::Path;

use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::model::{QueueItem, RedemptionRef, Source, Track};

type Result<T> = std::result::Result<T, DbError>;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("duplicate redemption")]
    Duplicate,
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RedemptionState {
    Unknown,
    Queued,
    Started,
    Cancelled,
}

pub struct Db(rusqlite::Connection);

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let connection = rusqlite::Connection::open(path)?;
        connection.execute_batch(
            "
            BEGIN;
            CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item TEXT NOT NULL,
                redemption_id TEXT UNIQUE,
                position INTEGER NOT NULL,
                is_current INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item TEXT NOT NULL,
                redemption_id TEXT,
                played_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS cancelled (redemption_id TEXT PRIMARY KEY);
            PRAGMA user_version=1;
            COMMIT;
        ",
        )?;
        Ok(Db(connection))
    }

    pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value: Option<String> = self
            .0
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(value.map(|v| serde_json::from_str(&v)).transpose()?)
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, v: &T) -> Result<()> {
        self.0.execute(
            "INSERT INTO settings(key,value) VALUES (?1,?2)
            ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, serde_json::to_string(v)?],
        )?;
        Ok(())
    }

    pub fn insert_item(
        &self,
        track: &Track,
        requester: &str,
        r: Option<&RedemptionRef>,
    ) -> Result<QueueItem> {
        let tx = self.0.unchecked_transaction()?;
        if let Some(r) = r {
            let exists: bool = tx.query_row(
                "SELECT
                EXISTS(SELECT 1 FROM queue WHERE redemption_id=?1) OR
                EXISTS(SELECT 1 FROM history WHERE redemption_id=?1) OR
                EXISTS(SELECT 1 FROM cancelled WHERE redemption_id=?1)",
                [&r.redemption_id],
                |row| row.get(0),
            )?;
            if exists {
                return Err(DbError::Duplicate);
            }
        }
        let mut item = QueueItem {
            id: 0,
            track: track.clone(),
            requester: requester.into(),
            redemption: r.cloned(),
            started: false,
        };
        tx.execute(
            "INSERT INTO queue(item,redemption_id,position)
            VALUES (?1,?2,(SELECT COALESCE(MAX(position),-1)+1 FROM queue))",
            params![serde_json::to_string(&item)?, r.map(|r| &r.redemption_id)],
        )?;
        item.id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE queue SET item=?1 WHERE id=?2",
            params![serde_json::to_string(&item)?, item.id],
        )?;
        tx.commit()?;
        Ok(item)
    }

    pub fn save_state(&self, current: Option<&QueueItem>, queue: &[QueueItem]) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute("DELETE FROM queue", [])?;
        for (is_current, items) in [
            (true, current.into_iter().collect::<Vec<_>>()),
            (false, queue.iter().collect()),
        ] {
            for (position, item) in items.into_iter().enumerate() {
                tx.execute("INSERT INTO queue(id,item,redemption_id,position,is_current) VALUES (?1,?2,?3,?4,?5)",
                    params![item.id, serde_json::to_string(item)?, item.redemption.as_ref().map(|r| &r.redemption_id), position as i64, is_current])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_state(&self) -> Result<(Option<QueueItem>, Vec<QueueItem>)> {
        let mut statement = self
            .0
            .prepare("SELECT item,is_current FROM queue ORDER BY position")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })?;
        let mut current = None;
        let mut queue = Vec::new();
        for row in rows {
            let (json, is_current) = row?;
            let item = serde_json::from_str(&json)?;
            if is_current {
                current = Some(item);
            } else {
                queue.push(item);
            }
        }
        Ok((current, queue))
    }

    pub fn history_push(&self, item: &QueueItem, now: i64) -> Result<()> {
        self.0.execute(
            "INSERT INTO history(item,redemption_id,played_at) VALUES (?1,?2,?3)",
            params![
                serde_json::to_string(item)?,
                item.redemption.as_ref().map(|r| &r.redemption_id),
                now
            ],
        )?;
        Ok(())
    }

    pub fn history_recent(&self, n: usize) -> Result<Vec<Source>> {
        let mut statement = self
            .0
            .prepare("SELECT item FROM history ORDER BY played_at DESC,id DESC LIMIT ?1")?;
        let rows = statement.query_map([i64::try_from(n).unwrap_or(i64::MAX)], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str::<QueueItem>(&row?)?.track.source))
            .collect()
    }

    pub fn redemption_state(&self, redemption_id: &str) -> Result<RedemptionState> {
        let cancelled: bool = self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM cancelled WHERE redemption_id=?1)",
            [redemption_id],
            |row| row.get(0),
        )?;
        if cancelled {
            return Ok(RedemptionState::Cancelled);
        }
        let historical: bool = self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM history WHERE redemption_id=?1)",
            [redemption_id],
            |row| row.get(0),
        )?;
        if historical {
            return Ok(RedemptionState::Started);
        }
        let item: Option<(String, bool)> = self
            .0
            .query_row(
                "SELECT item,is_current FROM queue WHERE redemption_id=?1",
                [redemption_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match item {
            Some((json, current)) => {
                let item: QueueItem = serde_json::from_str(&json)?;
                Ok(if current && item.started {
                    RedemptionState::Started
                } else {
                    RedemptionState::Queued
                })
            }
            None => Ok(RedemptionState::Unknown),
        }
    }

    pub fn mark_cancelled(&self, redemption_id: &str) -> Result<()> {
        self.0.execute(
            "INSERT OR IGNORE INTO cancelled(redemption_id) VALUES (?1)",
            [redemption_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RedemptionRef, Source, Track};

    fn track() -> Track {
        Track {
            source: Source::Spotify {
                uri: "spotify:track:test".into(),
            },
            title: "Title".into(),
            artist: "Artist".into(),
            duration_ms: Some(1000),
            explicit: Some(false),
            cover_url: None,
        }
    }
    fn redemption() -> RedemptionRef {
        RedemptionRef {
            reward_id: "reward".into(),
            redemption_id: "redemption".into(),
        }
    }
    fn memory() -> Db {
        Db::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn insert_assigns_ids_in_order() {
        let db = memory();
        let a = db.insert_item(&track(), "a", None).unwrap();
        let b = db.insert_item(&track(), "b", None).unwrap();
        assert!(a.id > 0 && b.id > a.id);
        assert!(!a.started);
        assert_eq!(db.load_state().unwrap(), (None, vec![a, b]));
    }
    #[test]
    fn duplicate_redemption_rejected() {
        let db = memory();
        db.insert_item(&track(), "a", Some(&redemption())).unwrap();
        assert!(matches!(
            db.insert_item(&track(), "b", Some(&redemption())),
            Err(DbError::Duplicate)
        ));
    }
    #[test]
    fn save_state_reorders_and_deletes() {
        let db = memory();
        let a = db.insert_item(&track(), "a", None).unwrap();
        let b = db.insert_item(&track(), "b", None).unwrap();
        let c = db.insert_item(&track(), "c", None).unwrap();
        db.save_state(None, &[c.clone(), a.clone()]).unwrap();
        assert_eq!(db.load_state().unwrap(), (None, vec![c.clone(), a.clone()]));
        assert_eq!(
            db.0.query_row("SELECT count(*) FROM queue WHERE id=?1", [b.id], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        db.save_state(Some(&a), &[c.clone()]).unwrap();
        assert_eq!(db.load_state().unwrap(), (Some(a), vec![c]));
    }
    #[test]
    fn state_survives_reopen() {
        let path = std::env::temp_dir().join(format!(
            "stream-assist-db-{}-{}.sqlite",
            std::process::id(),
            rand::random::<u64>()
        ));
        let expected;
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(
                db.0.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            let mut a = db.insert_item(&track(), "a", Some(&redemption())).unwrap();
            a.started = true;
            let b = db.insert_item(&track(), "b", None).unwrap();
            db.save_state(Some(&a), &[b.clone()]).unwrap();
            expected = (Some(a), vec![b]);
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.load_state().unwrap(), expected);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn redemption_state_tracks_lifecycle() {
        let db = memory();
        let r = redemption();
        assert_eq!(
            db.redemption_state(&r.redemption_id).unwrap(),
            RedemptionState::Unknown
        );
        let mut a = db.insert_item(&track(), "a", Some(&r)).unwrap();
        assert_eq!(
            db.redemption_state(&r.redemption_id).unwrap(),
            RedemptionState::Queued
        );
        a.started = true;
        db.save_state(Some(&a), &[]).unwrap();
        assert_eq!(
            db.redemption_state(&r.redemption_id).unwrap(),
            RedemptionState::Started
        );
        db.history_push(&a, 10).unwrap();
        db.save_state(None, &[]).unwrap();
        assert_eq!(
            db.redemption_state(&r.redemption_id).unwrap(),
            RedemptionState::Started
        );
        assert_eq!(db.history_recent(1).unwrap(), vec![a.track.source]);
        assert!(db.history_recent(0).unwrap().is_empty());
        db.mark_cancelled("cancelled").unwrap();
        assert_eq!(
            db.redemption_state("cancelled").unwrap(),
            RedemptionState::Cancelled
        );
    }
    #[test]
    fn settings_roundtrip() {
        let db = memory();
        assert_eq!(db.get_setting::<Vec<String>>("key").unwrap(), None);
        db.set_setting("key", &vec!["value"]).unwrap();
        assert_eq!(
            db.get_setting::<Vec<String>>("key").unwrap(),
            Some(vec!["value".into()])
        );
        db.set_setting("key", &42).unwrap();
        assert_eq!(db.get_setting::<i64>("key").unwrap(), Some(42));
    }
}
