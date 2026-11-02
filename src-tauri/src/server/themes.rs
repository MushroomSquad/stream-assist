use anyhow::{Context, Result, bail};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path},
};
static BUILTIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/widget/themes");
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartSize {
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThemeParts {
    pub now: PartSize,
    pub queue: PartSize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThemeMeta {
    pub id: String,
    pub name: String,
    pub author: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub parts: Option<ThemeParts>,
}
fn safe(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && !path.split('/').any(|s| s == ".." || s == ".")
}
fn valid_id(id: &str) -> bool {
    safe(id) && !id.contains('/')
}
pub(super) fn asset(dir: &Path, id: &str, path: &str) -> Option<Vec<u8>> {
    if !valid_id(id) || !safe(path) {
        return None;
    }
    let root = dir.join(id);
    if root.is_dir() {
        let canonical = root.canonicalize().ok()?;
        let target = root.join(path).canonicalize().ok()?;
        if !target.starts_with(canonical) {
            return None;
        }
        fs::read(target).ok()
    } else {
        BUILTIN
            .get_file(format!("{id}/{path}"))
            .map(|f| f.contents().to_vec())
    }
}
pub fn list_themes(dir: &Path) -> Vec<ThemeMeta> {
    let mut all = BTreeMap::new();
    for d in BUILTIN.dirs() {
        if let Some(f) = d.get_file(d.path().join("theme.json")) {
            if let Ok(m) = serde_json::from_slice::<ThemeMeta>(f.contents()) {
                all.insert(m.id.clone(), m);
            }
        }
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(data) = fs::read(entry.path().join("theme.json")) {
                if let Ok(m) = serde_json::from_slice::<ThemeMeta>(&data) {
                    if valid_id(&m.id) && entry.file_name() == m.id.as_str() {
                        all.insert(m.id.clone(), m);
                    }
                }
            }
        }
    }
    all.into_values()
        .map(|mut theme| {
            if theme.parts.is_none() {
                let size = PartSize {
                    width: theme.width,
                    height: theme.height,
                };
                theme.parts = Some(ThemeParts {
                    now: size.clone(),
                    queue: size,
                });
            }
            theme
        })
        .collect()
}
pub fn import_theme_zip(zip: &Path, themes_dir: &Path) -> Result<String> {
    let mut archive = zip::ZipArchive::new(fs::File::open(zip)?)?;
    // Validate every entry before creating any theme files.
    for n in 0..archive.len() {
        let f = archive.by_index(n)?;
        let name = f.name().trim_end_matches('/');
        if !safe(name) || f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            bail!("Unsafe theme path")
        }
    }
    let meta: ThemeMeta = {
        let f = archive
            .by_name("theme.json")
            .context("Missing theme.json")?;
        serde_json::from_reader(f)?
    };
    if !valid_id(&meta.id) {
        bail!("Invalid theme id")
    }
    let root = themes_dir.join(&meta.id);
    if root.exists() {
        bail!("Theme already exists")
    }
    fs::create_dir_all(themes_dir)?;
    fs::create_dir(&root)?;
    let result = (|| -> Result<()> {
        for n in 0..archive.len() {
            let mut f = archive.by_index(n)?;
            let target = root.join(f.name());
            if f.is_dir() {
                fs::create_dir_all(target)?;
            } else {
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p)?
                }
                std::io::copy(&mut f, &mut fs::File::create(target)?)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&root);
        return Err(e);
    }
    Ok(meta.id)
}
