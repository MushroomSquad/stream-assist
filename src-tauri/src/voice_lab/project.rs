use super::{
    VlError,
    commands::{check_export_path, validate_params},
    err,
    mixer::Recipe,
    models::MODEL_REVISION,
    store::{RenderParams, Store},
    style::{canonical_bytes, parse_style, sha256_hex},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const MANIFEST_LIMIT: u64 = 1024 * 1024;
const PROFILE_LIMIT: u64 = 2 * 1024 * 1024;
const WAV_LIMIT: u64 = 50 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 500 * 1024 * 1024;
const ENTRY_LIMIT: usize = 2000;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    kind: String,
    schema_version: u32,
    asset_revision: String,
    created_at: String,
    profiles: Vec<ProjectProfile>,
    renders: Vec<ProjectRender>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectProfile {
    file: String,
    sha256: String,
    name: String,
    tags: Vec<String>,
    origin: String,
    recipe: Option<Recipe>,
    meta: Option<Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRender {
    file: String,
    #[serde(flatten)]
    params: RenderParams,
    recipe: Option<Recipe>,
    profile_sha256: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub profiles: usize,
    pub renders: usize,
    pub skipped: usize,
    pub revision_mismatch: bool,
}

fn invalid() -> VlError {
    err("PROJECT_INVALID")
}
fn profile_name(name: &str) -> bool {
    name.strip_prefix("profiles/")
        .and_then(|n| n.strip_suffix(".json"))
        .is_some_and(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
fn render_name(name: &str) -> bool {
    name.strip_prefix("renders/")
        .and_then(|n| n.strip_suffix(".wav"))
        .is_some_and(|s| (1..=9).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
}
fn validate_recipe(recipe: &Option<Recipe>) -> Result<(), VlError> {
    if recipe.as_ref().is_some_and(|r| {
        r.schema_version != 1
            || r.kind != "voice-lab-recipe"
            || !(1..=8).contains(&r.inputs.len())
            || r.inputs
                .iter()
                .any(|i| !i.weight.is_finite() || !(0.0..=100.0).contains(&i.weight))
    }) {
        return Err(invalid());
    }
    Ok(())
}
fn validate_manifest(m: &Manifest) -> Result<(), VlError> {
    if m.kind != "voice-lab-project" || m.schema_version != 1 {
        return Err(invalid());
    }
    if m.profiles.len() + m.renders.len() + 1 > ENTRY_LIMIT {
        return Err(err("PROJECT_TOO_LARGE"));
    }
    let mut names = HashSet::new();
    for p in &m.profiles {
        if !profile_name(&p.file)
            || !names.insert(&p.file)
            || p.name.chars().count() > 200
            || p.tags.len() > 20
            || p.tags.iter().any(|t| t.chars().count() > 50)
        {
            return Err(invalid());
        }
        validate_recipe(&p.recipe)?;
    }
    for r in &m.renders {
        if !render_name(&r.file) || !names.insert(&r.file) {
            return Err(invalid());
        }
        validate_recipe(&r.recipe)?;
        validate_params(&r.params).map_err(|_| invalid())?;
    }
    Ok(())
}

// Count bytes returned by the decompressor, never just the ZIP size fields.
fn copy_limited(
    reader: impl Read,
    writer: &mut impl Write,
    limit: u64,
    total: &mut u64,
) -> Result<(), VlError> {
    let allowed = limit.min(TOTAL_LIMIT.saturating_sub(*total));
    let n = std::io::copy(&mut reader.take(allowed + 1), writer).map_err(|_| invalid())?;
    if n > allowed {
        return Err(err("PROJECT_TOO_LARGE"));
    }
    *total += n;
    Ok(())
}
fn read_entry(
    z: &mut zip::ZipArchive<File>,
    name: &str,
    limit: u64,
    total: &mut u64,
) -> Result<Vec<u8>, VlError> {
    let mut bytes = Vec::new();
    copy_limited(
        z.by_name(name).map_err(|_| invalid())?,
        &mut bytes,
        limit,
        total,
    )?;
    Ok(bytes)
}
struct ImportDir(PathBuf);
impl Drop for ImportDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn import_project(store: &Mutex<Store>, src: &Path) -> Result<ImportReport, VlError> {
    let file = File::open(src).map_err(|_| invalid())?;
    if file.metadata().map_err(|_| invalid())?.len() > TOTAL_LIMIT {
        return Err(err("PROJECT_TOO_LARGE"));
    }
    let mut directory = file.try_clone().map_err(|_| invalid())?;
    let mut z = zip::ZipArchive::new(file).map_err(|_| invalid())?;
    // zip 6 deduplicates names internally: len() is not the raw entry count.
    // Walk only central-directory headers, without opening any entry by index.
    directory
        .seek(SeekFrom::Start(z.central_directory_start()))
        .map_err(|_| invalid())?;
    let mut count = 0;
    loop {
        let mut signature = [0; 4];
        directory
            .read_exact(&mut signature)
            .map_err(|_| invalid())?;
        if signature != *b"PK\x01\x02" {
            break;
        }
        count += 1;
        if count > ENTRY_LIMIT {
            return Err(err("PROJECT_TOO_LARGE"));
        }
        let mut header = [0; 42];
        directory.read_exact(&mut header).map_err(|_| invalid())?;
        let variable_len: u64 = [24, 26, 28]
            .into_iter()
            .map(|i| u64::from(u16::from_le_bytes([header[i], header[i + 1]])))
            .sum();
        directory
            .seek(SeekFrom::Current(variable_len as i64))
            .map_err(|_| invalid())?;
    }
    if z.len() > ENTRY_LIMIT {
        return Err(err("PROJECT_TOO_LARGE"));
    }
    if z.file_names()
        .any(|n| n != "manifest.json" && !profile_name(n) && !render_name(n))
    {
        return Err(invalid());
    }
    let mut total = 0;
    let m: Manifest = serde_json::from_slice(&read_entry(
        &mut z,
        "manifest.json",
        MANIFEST_LIMIT,
        &mut total,
    )?)
    .map_err(|_| invalid())?;
    validate_manifest(&m)?;
    let root = store.lock().map_err(|_| invalid())?.root().to_path_buf();
    let path = root.join(format!("tmp/import-{:016x}", rand::random::<u64>()));
    fs::create_dir(&path).map_err(|_| invalid())?;
    let dir = ImportDir(path);
    let mut styles = Vec::new();
    for p in &m.profiles {
        let bytes = read_entry(&mut z, &p.file, PROFILE_LIMIT, &mut total)?;
        let style = parse_style(&bytes).map_err(|_| invalid())?;
        let sha = sha256_hex(&canonical_bytes(&style));
        if sha != p.sha256 || p.file != format!("profiles/{sha}.json") {
            return Err(invalid());
        }
        styles.push(style);
    }
    let mut durations = Vec::new();
    for (i, r) in m.renders.iter().enumerate() {
        let path = dir.0.join(format!("{i}.wav"));
        let mut out = File::create(&path).map_err(|_| invalid())?;
        copy_limited(
            z.by_name(&r.file).map_err(|_| invalid())?,
            &mut out,
            WAV_LIMIT,
            &mut total,
        )?;
        drop(out);
        let mut wav = hound::WavReader::open(&path).map_err(|_| invalid())?;
        let spec = wav.spec();
        if spec.channels != 1
            || spec.bits_per_sample != 16
            || spec.sample_format != hound::SampleFormat::Int
            || !(8000..=48000).contains(&spec.sample_rate)
        {
            return Err(invalid());
        }
        // Iterating also rejects truncated or malformed sample data.
        let mut samples = 0u64;
        for sample in wav.samples::<i16>() {
            sample.map_err(|_| invalid())?;
            samples += 1;
        }
        if samples != u64::from(wav.duration()) {
            return Err(invalid());
        }
        durations.push((samples * 1000 / u64::from(spec.sample_rate)) as i64);
    }
    let mut report = ImportReport {
        profiles: 0,
        renders: 0,
        skipped: 0,
        revision_mismatch: m.asset_revision != MODEL_REVISION,
    };
    let s = store.lock().map_err(|_| err("IO_FAILED"))?;
    for (p, style) in m.profiles.iter().zip(styles) {
        if s.profile_by_sha(&p.sha256)?.is_some() {
            report.skipped += 1;
            continue;
        }
        let mut meta = match p.meta.clone() {
            Some(Value::Object(o)) => o,
            other => {
                let mut o = serde_json::Map::new();
                o.insert("original".into(), other.unwrap_or(Value::Null));
                o
            }
        };
        meta.remove("sourceId");
        meta.insert("importedOrigin".into(), Value::String(p.origin.clone()));
        let profile = s.put_profile(
            &style,
            &p.name,
            "import",
            p.recipe.as_ref(),
            Some(Value::Object(meta)),
        )?;
        s.rename_profile(profile.id, &p.name, &p.tags)?;
        report.profiles += 1;
    }
    for (i, r) in m.renders.iter().enumerate() {
        let profile = r
            .profile_sha256
            .as_deref()
            .map(|sha| s.profile_by_sha(sha))
            .transpose()?
            .flatten();
        let render = s.add_render(
            profile.map(|p| p.id),
            r.recipe.as_ref(),
            &r.params,
            durations[i],
            0,
        )?;
        if let Err(e) = fs::rename(dir.0.join(format!("{i}.wav")), s.render_path(render.id)) {
            s.rollback_render(render.id)?;
            return Err(e.into());
        }
        s.set_favorite(render.id, true)?;
        report.renders += 1;
    }
    Ok(report)
}

pub fn export_project(store: &Mutex<Store>, dest: &Path) -> Result<(), VlError> {
    check_export_path(dest, "svlab")?;
    let (m, paths) = {
        let s = store.lock().map_err(|_| err("EXPORT_FAILED"))?;
        let mut profiles = Vec::new();
        let mut renders = Vec::new();
        let mut paths = Vec::new();
        for p in s.profiles()? {
            if p.origin == "builtin" {
                continue;
            }
            paths.push(s.profile_path(&p));
            profiles.push(ProjectProfile {
                file: format!("profiles/{}.json", p.style_sha256),
                sha256: p.style_sha256,
                name: p.name,
                tags: p.tags,
                origin: p.origin,
                recipe: p.recipe,
                meta: p.meta,
            });
        }
        for r in s.renders(usize::MAX)?.into_iter().filter(|r| r.favorite) {
            paths.push(s.render_path(r.id));
            let profile_sha256 = r
                .profile_id
                .map(|id| s.profile(id))
                .transpose()?
                .map(|p| p.style_sha256);
            renders.push(ProjectRender {
                file: format!("renders/{}.wav", r.id),
                params: RenderParams {
                    text: r.text,
                    lang: r.lang,
                    speed: r.speed,
                    quality: r.quality,
                },
                recipe: r.recipe,
                profile_sha256,
            });
        }
        (
            Manifest {
                kind: "voice-lab-project".into(),
                schema_version: 1,
                asset_revision: MODEL_REVISION.into(),
                created_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .to_string(),
                profiles,
                renders,
            },
            paths,
        )
    };
    validate_manifest(&m).map_err(|e| {
        if e.code() == "PROJECT_TOO_LARGE" {
            e
        } else {
            err("EXPORT_FAILED")
        }
    })?;
    let manifest = serde_json::to_vec(&m)?;
    if manifest.len() as u64 > MANIFEST_LIMIT {
        return Err(err("PROJECT_TOO_LARGE"));
    }
    let mut tmp = dest.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let file = File::create(&tmp).map_err(|_| err("EXPORT_FAILED"))?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("manifest.json", opts)
            .map_err(|_| err("EXPORT_FAILED"))?;
        zip.write_all(&manifest).map_err(|_| err("EXPORT_FAILED"))?;
        let mut total = manifest.len() as u64;
        let entries = m
            .profiles
            .iter()
            .map(|p| (&p.file, PROFILE_LIMIT))
            .chain(m.renders.iter().map(|r| (&r.file, WAV_LIMIT)));
        for ((name, limit), path) in entries.zip(paths) {
            zip.start_file(name, opts)
                .map_err(|_| err("EXPORT_FAILED"))?;
            let file = File::open(path).map_err(|_| err("EXPORT_FAILED"))?;
            copy_limited(file, &mut zip, limit, &mut total).map_err(|e| {
                if e.code() == "PROJECT_TOO_LARGE" {
                    e
                } else {
                    err("EXPORT_FAILED")
                }
            })?;
        }
        let file = zip.finish().map_err(|_| err("EXPORT_FAILED"))?;
        if file.metadata().map_err(|_| err("EXPORT_FAILED"))?.len() > TOTAL_LIMIT {
            return Err(err("PROJECT_TOO_LARGE"));
        }
        drop(file);
        fs::rename(&tmp, dest).map_err(|_| err("EXPORT_FAILED"))?;
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompressed_total_limit() {
        let mut total = TOTAL_LIMIT - 3;
        copy_limited(&b"abc"[..], &mut std::io::sink(), WAV_LIMIT, &mut total).unwrap();
        assert_eq!(total, TOTAL_LIMIT);
        assert_eq!(
            copy_limited(&b"x"[..], &mut std::io::sink(), WAV_LIMIT, &mut total)
                .unwrap_err()
                .code(),
            "PROJECT_TOO_LARGE"
        );
    }
}
