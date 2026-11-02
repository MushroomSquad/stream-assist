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
    style::{canonical_bytes, read_style_file, sha256_hex},
};

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
            "tmp",
        ] {
            fs::create_dir_all(root.join(sub))?;
        }
        for entry in fs::read_dir(root.join("tmp"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        let conn = Connection::open(root.join("voice-lab.db"))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;
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
            COMMIT;")?;
        Ok(Store {
            conn,
            root: root.to_path_buf(),
        })
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
        let path = self.profile_path(&profile);
        let style = read_style_file(&path)?;
        if sha256_hex(&fs::read(path)?) != profile.style_sha256 {
            return Err(err("PROFILE_HASH_MISMATCH"));
        }
        Ok(style)
    }

    pub fn profile_path(&self, profile: &Profile) -> PathBuf {
        self.root
            .join(format!("profiles/{}.json", profile.style_sha256))
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
            1
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
