use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use supertonic3_tts::Style;

use super::{
    VlError, err,
    mixer::Recipe,
    style::{canonical_bytes, load_verified, sha256_hex},
};

// Append migrations: index + 1 is the schema version.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sources (
                id INTEGER PRIMARY KEY, original_sha256 TEXT UNIQUE NOT NULL,
                ext TEXT, name TEXT, duration_ms INTEGER, consent INTEGER, peaks TEXT, created_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS profiles (
                id INTEGER PRIMARY KEY, name TEXT, tags TEXT, style_sha256 TEXT UNIQUE NOT NULL,
                origin TEXT, recipe_json TEXT, meta_json TEXT, created_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS renders (
                id INTEGER PRIMARY KEY, profile_id INTEGER REFERENCES profiles(id) ON DELETE CASCADE,
                recipe_json TEXT, text TEXT, lang TEXT, speed REAL, quality INTEGER,
                duration_ms INTEGER, wall_ms INTEGER, favorite INTEGER DEFAULT 0, created_at INTEGER
            );",
    "CREATE TABLE IF NOT EXISTS training_runs (
        id INTEGER PRIMARY KEY,
        source_id INTEGER REFERENCES sources(id) ON DELETE SET NULL,
        start_ms INTEGER, end_ms INTEGER, lang TEXT, params_json TEXT NOT NULL,
        status TEXT NOT NULL, step INTEGER DEFAULT 0, total INTEGER, best_loss REAL,
        history_json TEXT DEFAULT '[]', error TEXT, created_at INTEGER, updated_at INTEGER
    );",
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrainParams {
    pub preset: String,
    pub total_steps: u32,
    pub lr: f64,
    pub save_every: u32,
    pub seed: u64,
    pub threshold: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrainRun {
    pub id: i64,
    pub source_id: Option<i64>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub lang: String,
    pub params: TrainParams,
    pub status: String,
    pub step: i64,
    pub total: i64,
    pub best_loss: Option<f64>,
    pub history: Vec<[f64; 3]>,
    pub error: Option<String>,
    #[serde(skip_deserializing)]
    pub lr: Option<f64>,
    pub created_at: i64,
    pub updated_at: i64,
}

type Result<T> = std::result::Result<T, VlError>;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RenderParams {
    pub text: String,
    pub lang: String,
    pub speed: f32,
    pub quality: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: i64,
    pub name: String,
    pub duration_ms: i64,
    pub consent: bool,
    pub created_at: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: i64,
    pub name: String,
    pub tags: Vec<String>,
    pub style_sha256: String,
    pub origin: String,
    pub recipe: Option<Recipe>,
    pub meta: Option<Value>,
    pub created_at: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Render {
    pub id: i64,
    pub profile_id: Option<i64>,
    pub recipe: Option<Recipe>,
    pub text: String,
    pub lang: String,
    pub speed: f32,
    pub quality: usize,
    pub duration_ms: i64,
    pub wall_ms: i64,
    pub favorite: bool,
    pub created_at: i64,
}

pub struct Store {
    conn: Connection,
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> Result<Store> {
        for sub in [
            "sources/original",
            "sources/work",
            "profiles",
            "renders",
            "models",
            "training",
            "tmp",
        ] {
            fs::create_dir_all(root.join(sub))?;
        }
        // Best effort: a file still held by a dying worker must not block startup.
        for entry in fs::read_dir(root.join("tmp"))?.flatten() {
            let path = entry.path();
            let _ = if path.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            };
        }
        let conn = Connection::open(root.join("voice-lab.db"))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 0 || version > MIGRATIONS.len() as i64 {
            return Err(err("IO_FAILED"));
        }
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let tx = conn.unchecked_transaction()?;
        for migration in &MIGRATIONS[version as usize..] {
            tx.execute_batch(migration)?;
        }
        tx.pragma_update(None, "user_version", MIGRATIONS.len() as i64)?;
        tx.commit()?;
        conn.execute(
            "UPDATE training_runs SET status='interrupted' WHERE status='running'",
            [],
        )?;
        Ok(Store {
            conn,
            root: root.to_path_buf(),
        })
    }

    pub fn create_run(
        &self,
        source_id: Option<i64>,
        start_ms: i64,
        end_ms: i64,
        lang: &str,
        params: &TrainParams,
    ) -> Result<TrainRun> {
        let timestamp = now();
        self.conn.execute(
            "INSERT INTO training_runs (source_id,start_ms,end_ms,lang,params_json,status,total,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,'running',?6,?7,?7)",
            params![source_id,start_ms,end_ms,lang,serde_json::to_string(params)?,params.total_steps,timestamp],
        )?;
        self.run(self.conn.last_insert_rowid())
    }

    pub fn run(&self, id: i64) -> Result<TrainRun> {
        self.conn
            .query_row(
                "SELECT * FROM training_runs WHERE id=?1",
                [id],
                train_run_row,
            )
            .optional()?
            .ok_or_else(|| err("NOT_FOUND"))
    }

    pub fn runs(&self) -> Result<Vec<TrainRun>> {
        Ok(self
            .conn
            .prepare("SELECT * FROM training_runs ORDER BY created_at DESC, id DESC")?
            .query_map([], train_run_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_run_status(&self, id: i64, status: &str, error: Option<&str>) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE training_runs SET status=?2,error=?3,updated_at=?4 WHERE id=?1",
            params![id, status, error, now()],
        )?)
    }

    pub fn set_run_step(&self, id: i64, step: i64) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE training_runs SET step=?2,updated_at=?3 WHERE id=?1",
            params![id, step, now()],
        )?)
    }

    pub fn add_run_point(&self, id: i64, step: i64, loss: f64, best: f64) -> Result<()> {
        if !loss.is_finite() || !best.is_finite() {
            return Ok(());
        }
        let mut history = self.run(id)?.history;
        history.push([step as f64, loss, best]);
        changed(self.conn.execute(
            "UPDATE training_runs SET history_json=?2,updated_at=?3 WHERE id=?1",
            params![id, serde_json::to_string(&history)?, now()],
        )?)
    }

    pub fn truncate_run_history(&self, id: i64, max_step: i64) -> Result<()> {
        let mut history = self.run(id)?.history;
        history.retain(|point| point[0] <= max_step as f64);
        changed(self.conn.execute(
            "UPDATE training_runs SET history_json=?2,updated_at=?3 WHERE id=?1",
            params![id, serde_json::to_string(&history)?, now()],
        )?)
    }

    pub fn set_run_best(&self, id: i64, best: f64) -> Result<()> {
        if !best.is_finite() {
            return Ok(());
        }
        changed(self.conn.execute(
            "UPDATE training_runs SET best_loss=?2,updated_at=?3 WHERE id=?1",
            params![id, best, now()],
        )?)
    }

    pub fn delete_run(&self, id: i64) -> Result<()> {
        self.run(id)?;
        match fs::remove_dir_all(self.run_dir(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        changed(
            self.conn
                .execute("DELETE FROM training_runs WHERE id=?1", [id])?,
        )
    }

    pub fn run_dir(&self, id: i64) -> PathBuf {
        self.root.join(format!("training/{id}"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn find_source_by_sha(&self, sha: &str) -> Result<Option<Source>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM sources WHERE original_sha256=?1",
                [sha],
                source_row,
            )
            .optional()?)
    }

    pub fn add_source(
        &self,
        original_sha256: &str,
        ext: &str,
        name: &str,
        duration_ms: i64,
        consent: bool,
        peaks: &[f32],
    ) -> Result<Source> {
        if let Some(source) = self.find_source_by_sha(original_sha256)? {
            return Ok(source);
        }
        self.conn.execute("INSERT INTO sources (original_sha256,ext,name,duration_ms,consent,peaks,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![original_sha256,ext,name,duration_ms,consent,serde_json::to_string(peaks)?,now()])?;
        self.source(self.conn.last_insert_rowid())
    }

    pub(crate) fn rollback_source(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM sources WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn source_peaks(&self, id: i64) -> Result<Vec<f32>> {
        let json: String = self
            .conn
            .query_row("SELECT peaks FROM sources WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| err("NOT_FOUND"))?;
        Ok(serde_json::from_str(&json)?)
    }

    pub fn sources(&self) -> Result<Vec<Source>> {
        Ok(self
            .conn
            .prepare("SELECT * FROM sources ORDER BY created_at DESC, id DESC")?
            .query_map([], source_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn source(&self, id: i64) -> Result<Source> {
        self.conn
            .query_row("SELECT * FROM sources WHERE id=?1", [id], source_row)
            .optional()?
            .ok_or_else(|| err("NOT_FOUND"))
    }

    pub fn set_consent(&self, id: i64, consent: bool) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE sources SET consent=?2 WHERE id=?1",
            params![id, consent],
        )?)
    }

    pub fn source_work_path(&self, id: i64) -> PathBuf {
        self.root.join(format!("sources/work/{id}.wav"))
    }

    pub fn put_profile(
        &self,
        style: &Style,
        name: &str,
        origin: &str,
        recipe: Option<&Recipe>,
        meta: Option<Value>,
    ) -> Result<Profile> {
        let bytes = canonical_bytes(style);
        let sha = sha256_hex(&bytes);
        if let Some(profile) = self.profile_by_sha(&sha)? {
            return Ok(profile);
        }
        let recipe_json = recipe.map(serde_json::to_string).transpose()?;
        let meta_json = meta.map(|v| serde_json::to_string(&v)).transpose()?;
        let path = self.root.join(format!("profiles/{sha}.json"));
        let tmp = self.root.join(format!("tmp/profile-{sha}.json"));
        fs::write(&tmp, bytes)?;
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Err(e) = self.conn.execute("INSERT INTO profiles (name,tags,style_sha256,origin,recipe_json,meta_json,created_at) VALUES (?1,'[]',?2,?3,?4,?5,?6)",
            params![name,sha,origin,recipe_json,meta_json,now()]) {
            let _ = fs::remove_file(&path);
            return Err(e.into());
        }
        self.profile(self.conn.last_insert_rowid())
    }

    pub fn profiles(&self) -> Result<Vec<Profile>> {
        Ok(self
            .conn
            .prepare("SELECT * FROM profiles ORDER BY created_at DESC, id DESC")?
            .query_map([], profile_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn profile(&self, id: i64) -> Result<Profile> {
        self.conn
            .query_row("SELECT * FROM profiles WHERE id=?1", [id], profile_row)
            .optional()?
            .ok_or_else(|| err("NOT_FOUND"))
    }

    pub fn profile_by_sha(&self, sha: &str) -> Result<Option<Profile>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM profiles WHERE style_sha256=?1",
                [sha],
                profile_row,
            )
            .optional()?)
    }

    pub fn load_style(&self, id: i64) -> Result<Style> {
        let profile = self.profile(id)?;
        load_verified(&self.profile_path(&profile), &profile.style_sha256)
    }

    pub fn profile_path(&self, profile: &Profile) -> PathBuf {
        self.root
            .join(format!("profiles/{}.json", profile.style_sha256))
    }

    pub fn mark_builtin(&self, id: i64, name: &str) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE profiles SET origin='builtin',name=?2 WHERE id=?1",
            params![id, name],
        )?)
    }

    pub fn rename_profile(&self, id: i64, name: &str, tags: &[String]) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE profiles SET name=?2,tags=?3 WHERE id=?1",
            params![id, name, serde_json::to_string(tags)?],
        )?)
    }

    pub fn delete_profile(&self, id: i64) -> Result<()> {
        let profile = self.profile(id)?;
        if profile.origin == "builtin" {
            return Err(err("BUILTIN_LOCKED"));
        }
        let ids = self
            .conn
            .prepare("SELECT id FROM renders WHERE profile_id=?1")?
            .query_map([id], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for render_id in ids {
            remove_if_exists(&self.render_path(render_id))?;
        }
        self.conn
            .execute("DELETE FROM profiles WHERE id=?1", [id])?;
        remove_if_exists(&self.profile_path(&profile))
    }

    pub fn add_render(
        &self,
        profile_id: Option<i64>,
        recipe: Option<&Recipe>,
        p: &RenderParams,
        duration_ms: i64,
        wall_ms: i64,
    ) -> Result<Render> {
        self.conn.execute("INSERT INTO renders (profile_id,recipe_json,text,lang,speed,quality,duration_ms,wall_ms,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![profile_id,recipe.map(serde_json::to_string).transpose()?,p.text,p.lang,p.speed,p.quality as i64,duration_ms,wall_ms,now()])?;
        self.render(self.conn.last_insert_rowid())
    }

    pub(super) fn rollback_render(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM renders WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn render_path(&self, id: i64) -> PathBuf {
        self.root.join(format!("renders/{id}.wav"))
    }

    pub fn renders(&self, limit: usize) -> Result<Vec<Render>> {
        Ok(self
            .conn
            .prepare("SELECT * FROM renders ORDER BY created_at DESC, id DESC LIMIT ?1")?
            .query_map([limit.min(i64::MAX as usize) as i64], render_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn render(&self, id: i64) -> Result<Render> {
        self.conn
            .query_row("SELECT * FROM renders WHERE id=?1", [id], render_row)
            .optional()?
            .ok_or_else(|| err("NOT_FOUND"))
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE renders SET favorite=?2 WHERE id=?1",
            params![id, favorite],
        )?)
    }

    pub fn prune_renders(&self, keep: usize) -> Result<()> {
        let ids = self.conn.prepare("SELECT id FROM renders WHERE favorite=0 ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?1")?
            .query_map([keep.min(i64::MAX as usize) as i64], |r| r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            remove_if_exists(&self.render_path(id))?;
            self.conn.execute("DELETE FROM renders WHERE id=?1", [id])?;
        }
        Ok(())
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn changed(count: usize) -> Result<()> {
    if count == 0 {
        Err(err("NOT_FOUND"))
    } else {
        Ok(())
    }
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn json_column<T: serde::de::DeserializeOwned>(row: &Row<'_>, name: &str) -> rusqlite::Result<T> {
    let json: String = row.get(name)?;
    serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref().column_index(name).unwrap_or(0),
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })
}

fn optional_json_column<T: serde::de::DeserializeOwned>(
    row: &Row<'_>,
    name: &str,
) -> rusqlite::Result<Option<T>> {
    let json: Option<String> = row.get(name)?;
    json.map(|s| {
        serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                row.as_ref().column_index(name).unwrap_or(0),
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
    })
    .transpose()
}

fn train_run_row(row: &Row<'_>) -> rusqlite::Result<TrainRun> {
    Ok(TrainRun {
        id: row.get("id")?,
        source_id: row.get("source_id")?,
        start_ms: row.get("start_ms")?,
        end_ms: row.get("end_ms")?,
        lang: row.get("lang")?,
        params: json_column(row, "params_json")?,
        status: row.get("status")?,
        step: row.get("step")?,
        total: row.get("total")?,
        best_loss: row.get("best_loss")?,
        history: json_column(row, "history_json")?,
        error: row.get("error")?,
        lr: None,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn source_row(row: &Row<'_>) -> rusqlite::Result<Source> {
    Ok(Source {
        id: row.get("id")?,
        name: row.get("name")?,
        duration_ms: row.get("duration_ms")?,
        consent: row.get("consent")?,
        created_at: row.get("created_at")?,
    })
}

fn profile_row(row: &Row<'_>) -> rusqlite::Result<Profile> {
    Ok(Profile {
        id: row.get("id")?,
        name: row.get("name")?,
        tags: json_column(row, "tags")?,
        style_sha256: row.get("style_sha256")?,
        origin: row.get("origin")?,
        recipe: optional_json_column(row, "recipe_json")?,
        meta: optional_json_column(row, "meta_json")?,
        created_at: row.get("created_at")?,
    })
}

fn render_row(row: &Row<'_>) -> rusqlite::Result<Render> {
    let quality: i64 = row.get("quality")?;
    let quality = usize::try_from(quality)
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(6, quality))?;
    Ok(Render {
        id: row.get("id")?,
        profile_id: row.get("profile_id")?,
        recipe: optional_json_column(row, "recipe_json")?,
        text: row.get("text")?,
        lang: row.get("lang")?,
        speed: row.get("speed")?,
        quality,
        duration_ms: row.get("duration_ms")?,
        wall_ms: row.get("wall_ms")?,
        favorite: row.get("favorite")?,
        created_at: row.get("created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_lab::{
        mixer::{DpStrategy, new_recipe},
        style::{canonical_bytes, sha256_hex},
    };
    use ndarray::Array3;
    use supertonic3_tts::Style;
    use tempfile::tempdir;

    fn train_params() -> TrainParams {
        TrainParams {
            preset: "fast".into(),
            total_steps: 1000,
            lr: 2e-4,
            save_every: 500,
            seed: 42,
            threshold: 0.24,
        }
    }

    #[test]
    fn migration_from_v1_keeps_data() {
        let dir = tempdir().unwrap();
        // Frozen DDL from the P0 Store::open, independent of MIGRATIONS.
        let conn = Connection::open(dir.path().join("voice-lab.db")).unwrap();
        conn.execute_batch(r#"PRAGMA foreign_keys=ON;
            BEGIN;
            CREATE TABLE IF NOT EXISTS sources (
                id INTEGER PRIMARY KEY, original_sha256 TEXT UNIQUE NOT NULL,
                ext TEXT, name TEXT, duration_ms INTEGER, consent INTEGER, peaks TEXT, created_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS profiles (
                id INTEGER PRIMARY KEY, name TEXT, tags TEXT, style_sha256 TEXT UNIQUE NOT NULL,
                origin TEXT, recipe_json TEXT, meta_json TEXT, created_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS renders (
                id INTEGER PRIMARY KEY, profile_id INTEGER REFERENCES profiles(id) ON DELETE CASCADE,
                recipe_json TEXT, text TEXT, lang TEXT, speed REAL, quality INTEGER,
                duration_ms INTEGER, wall_ms INTEGER, favorite INTEGER DEFAULT 0, created_at INTEGER
            );
            PRAGMA user_version=1;
            COMMIT;"#).unwrap();
        for sub in ["profiles", "tmp", "sources/work", "renders"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let old = Store {
            conn,
            root: dir.path().to_path_buf(),
        };
        let source = old
            .add_source("legacy", "wav", "Legacy", 5000, true, &[0.5])
            .unwrap();
        let profile = old
            .put_profile(
                &style(),
                "Legacy",
                "clone",
                None,
                Some(serde_json::json!({"sourceId":source.id})),
            )
            .unwrap();
        let render = old
            .add_render(Some(profile.id), None, &params(), 1000, 500)
            .unwrap();
        fs::write(old.source_work_path(source.id), b"source").unwrap();
        fs::write(old.render_path(render.id), b"render").unwrap();
        drop(old);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.profile(profile.id).unwrap().name, "Legacy");
        assert_eq!(s.load_style(profile.id).unwrap().ttl, style().ttl);
        assert_eq!(s.source(source.id).unwrap().name, "Legacy");
        assert_eq!(s.source_peaks(source.id).unwrap(), vec![0.5]);
        assert_eq!(fs::read(s.source_work_path(source.id)).unwrap(), b"source");
        assert_eq!(s.render(render.id).unwrap().profile_id, Some(profile.id));
        assert_eq!(fs::read(s.render_path(render.id)).unwrap(), b"render");
        assert!(s.runs().unwrap().is_empty());
        assert_eq!(
            s.conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn fresh_db_gets_latest_version() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(
            s.conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            MIGRATIONS.len() as i64
        );
        assert!(s.root().join("training").is_dir());
        assert!(s.runs().unwrap().is_empty());
    }

    #[test]
    fn future_version_rejected() {
        let dir = tempdir().unwrap();
        let conn = Connection::open(dir.path().join("voice-lab.db")).unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        assert_eq!(Store::open(dir.path()).err().unwrap().code(), "IO_FAILED");
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            999
        );
    }

    #[test]
    fn startup_marks_running_interrupted() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let a = s.create_run(None, 0, 5000, "ru", &train_params()).unwrap();
        let b = s.create_run(None, 0, 5000, "en", &train_params()).unwrap();
        s.set_run_status(b.id, "failed", Some("GPU_OUT_OF_MEMORY"))
            .unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.run(a.id).unwrap().status, "interrupted");
        assert_eq!(s.run(b.id).unwrap().status, "failed");
        assert_eq!(
            s.run(b.id).unwrap().error.as_deref(),
            Some("GPU_OUT_OF_MEMORY")
        );
        s.set_run_status(b.id, "running", None).unwrap();
        assert!(s.run(b.id).unwrap().error.is_none());
    }

    #[test]
    fn history_points_and_truncate() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let r = s
            .create_run(None, 100, 5100, "ru", &train_params())
            .unwrap();
        assert_eq!(r.params, train_params());
        assert_eq!(
            (r.start_ms, r.end_ms, r.total, r.step),
            (100, 5100, 1000, 0)
        );
        assert_eq!(r.lang, "ru");
        assert_eq!(r.status, "running");
        assert!(r.lr.is_none());
        s.add_run_point(r.id, 8, 0.8, 0.7).unwrap();
        s.add_run_point(r.id, 16, 0.6, 0.5).unwrap();
        for (loss, best) in [
            (f64::NAN, 0.1),
            (0.1, f64::INFINITY),
            (f64::NEG_INFINITY, 0.1),
        ] {
            s.add_run_point(r.id, 24, loss, best).unwrap();
        }
        assert_eq!(
            s.run(r.id).unwrap().history,
            vec![[8., 0.8, 0.7], [16., 0.6, 0.5]]
        );
        s.truncate_run_history(r.id, 8).unwrap();
        s.set_run_step(r.id, 8).unwrap();
        s.set_run_best(r.id, 0.7).unwrap();
        let r = s.run(r.id).unwrap();
        assert_eq!(r.history, vec![[8., 0.8, 0.7]]);
        assert_eq!(r.step, 8);
        assert_eq!(r.best_loss, Some(0.7));
        assert!(r.updated_at >= r.created_at);
        assert_eq!(s.run(999).unwrap_err().code(), "NOT_FOUND");
    }

    #[test]
    fn delete_run_keeps_profiles() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let source = s.add_source("a", "wav", "A", 5000, true, &[]).unwrap();
        let r = s
            .create_run(Some(source.id), 0, 5000, "ru", &train_params())
            .unwrap();
        let p = s
            .put_profile(
                &style(),
                "Checkpoint",
                "train",
                None,
                Some(serde_json::json!({"runId":r.id})),
            )
            .unwrap();
        fs::create_dir_all(s.run_dir(r.id)).unwrap();
        fs::write(s.run_dir(r.id).join("state.pt"), b"state").unwrap();
        s.rollback_source(source.id).unwrap();
        assert_eq!(s.run(r.id).unwrap().source_id, None);
        s.delete_run(r.id).unwrap();
        assert!(!s.run_dir(r.id).exists());
        assert_eq!(s.run(r.id).unwrap_err().code(), "NOT_FOUND");
        assert_eq!(s.load_style(p.id).unwrap().ttl, style().ttl);
    }

    fn style() -> Style {
        Style {
            ttl: Array3::from_elem((1, 50, 256), 0.25),
            dp: Array3::from_elem((1, 8, 16), 0.5),
        }
    }
    fn params() -> RenderParams {
        RenderParams {
            text: "Проверка".into(),
            lang: "ru".into(),
            speed: 1.0,
            quality: 8,
        }
    }
    #[test]
    fn put_profile_dedups_by_sha() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let a = s.put_profile(&style(), "A", "import", None, None).unwrap();
        let b = s.put_profile(&style(), "B", "clone", None, None).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.name, "A");
        assert_eq!(s.profiles().unwrap().len(), 1);
        assert!(s.root().join("tmp").read_dir().unwrap().next().is_none());
    }
    #[test]
    fn profile_file_is_canonical() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let p = s.put_profile(&style(), "A", "import", None, None).unwrap();
        let bytes = std::fs::read(s.profile_path(&p)).unwrap();
        assert_eq!(bytes, canonical_bytes(&style()));
        assert_eq!(sha256_hex(&bytes), p.style_sha256);
        assert_eq!(s.load_style(p.id).unwrap().ttl, style().ttl);
    }
    #[test]
    fn rename_keeps_bytes() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let p = s.put_profile(&style(), "A", "import", None, None).unwrap();
        let before = std::fs::read(s.profile_path(&p)).unwrap();
        s.rename_profile(p.id, "B", &["tag".to_string()]).unwrap();
        let p = s.profile(p.id).unwrap();
        assert_eq!(p.name, "B");
        assert_eq!(p.tags, vec!["tag"]);
        assert_eq!(before, std::fs::read(s.profile_path(&p)).unwrap());
    }
    #[test]
    fn tampered_profile_hash_mismatch() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let p = s.put_profile(&style(), "A", "import", None, None).unwrap();
        let mut changed = style();
        changed.ttl.fill(0.75);
        std::fs::write(s.profile_path(&p), canonical_bytes(&changed)).unwrap();
        assert_eq!(
            s.load_style(p.id).unwrap_err().code(),
            "PROFILE_HASH_MISMATCH"
        );
    }
    #[test]
    fn source_dedup_by_sha() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let peaks = vec![0.5; 1000];
        let a = s
            .add_source("abc", "wav", "A", 1000, false, &peaks)
            .unwrap();
        let b = s.add_source("abc", "mp3", "B", 2000, true, &[]).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.name, "A");
        assert!(!b.consent);
        assert_eq!(s.source_peaks(a.id).unwrap(), peaks);
        assert_eq!(s.sources().unwrap().len(), 1);
        assert_eq!(s.find_source_by_sha("abc").unwrap().unwrap().id, a.id);
        assert!(s.find_source_by_sha("missing").unwrap().is_none());
        assert_eq!(
            s.source_work_path(a.id),
            dir.path().join(format!("sources/work/{}.wav", a.id))
        );
    }
    #[test]
    fn consent_toggle() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let a = s.add_source("abc", "wav", "A", 1000, false, &[]).unwrap();
        s.set_consent(a.id, true).unwrap();
        assert!(s.source(a.id).unwrap().consent);
        s.set_consent(a.id, false).unwrap();
        assert!(!s.source(a.id).unwrap().consent);
    }
    #[test]
    fn open_clears_tmp() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        std::fs::create_dir(s.root().join("tmp/nested")).unwrap();
        std::fs::write(s.root().join("tmp/nested/stale"), b"x").unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert!(s.root().join("tmp").read_dir().unwrap().next().is_none());
        for sub in [
            "sources/original",
            "sources/work",
            "profiles",
            "renders",
            "models",
        ] {
            assert!(s.root().join(sub).is_dir());
        }
        assert_eq!(
            s.conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            MIGRATIONS.len() as i64
        );
        assert_eq!(
            s.conn
                .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn prune_keeps_favorites() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        for i in 0..205 {
            let r = s.add_render(None, None, &params(), 1000, 500).unwrap();
            std::fs::write(s.render_path(r.id), b"wav").unwrap();
            if i < 3 {
                s.set_favorite(r.id, true).unwrap();
            }
        }
        s.prune_renders(200).unwrap();
        let renders = s.renders(300).unwrap();
        assert_eq!(renders.len(), 203);
        assert_eq!(renders.iter().filter(|r| r.favorite).count(), 3);
        for id in 1..=205 {
            assert_eq!(s.render_path(id).exists(), id != 4 && id != 5);
        }
        assert_eq!(
            s.renders(2)
                .unwrap()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![205, 204]
        );
        s.prune_renders(0).unwrap();
        assert_eq!(s.renders(300).unwrap().len(), 3);
    }
    #[test]
    fn delete_profile_cascades_renders() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let p = s.put_profile(&style(), "A", "import", None, None).unwrap();
        let r = s
            .add_render(Some(p.id), None, &params(), 1000, 500)
            .unwrap();
        std::fs::write(s.render_path(r.id), b"wav").unwrap();
        s.delete_profile(p.id).unwrap();
        assert!(s.profiles().unwrap().is_empty());
        assert!(s.renders(200).unwrap().is_empty());
        assert!(!s.render_path(r.id).exists());
        assert!(!s.profile_path(&p).exists());
    }
    #[test]
    fn builtin_not_deletable() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let p = s.put_profile(&style(), "A", "builtin", None, None).unwrap();
        assert_eq!(s.delete_profile(p.id).unwrap_err().code(), "BUILTIN_LOCKED");
        assert!(s.profile_path(&p).exists());
        assert_eq!(s.profile(p.id).unwrap().origin, "builtin");
    }
    #[test]
    fn state_survives_reopen() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let recipe = new_recipe(vec![], DpStrategy::Dominant, None, "today".into());
        let meta = serde_json::json!({"sourceId":42});
        let p = s
            .put_profile(&style(), "A", "mix", Some(&recipe), Some(meta.clone()))
            .unwrap();
        s.rename_profile(p.id, "B", &["tag".into()]).unwrap();
        let a = s.add_source("abc", "wav", "A", 1000, true, &[0.5]).unwrap();
        let r = s
            .add_render(Some(p.id), Some(&recipe), &params(), 1200, 345)
            .unwrap();
        s.set_favorite(r.id, true).unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        let p = s.profile(p.id).unwrap();
        assert_eq!(p.recipe, Some(recipe.clone()));
        assert_eq!(p.meta, Some(meta));
        assert_eq!(p.tags, vec!["tag"]);
        assert_eq!(s.profile_by_sha(&p.style_sha256).unwrap().unwrap().id, p.id);
        assert!(s.source(a.id).unwrap().consent);
        assert_eq!(s.source_peaks(a.id).unwrap(), vec![0.5]);
        let r = s.render(r.id).unwrap();
        assert!(r.favorite);
        assert_eq!(r.recipe, Some(recipe));
        assert_eq!(r.wall_ms, 345);
        assert_eq!(r.duration_ms, 1200);
        assert_eq!(r.text, params().text);
    }
}
